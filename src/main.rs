// StockWise is an application crate. The lints below are silenced deliberately:
// - dead_code: helpers, broker-response fields, and stubs kept for upcoming
//   ROADMAP features and to document the shape of deserialized API responses.
// - the clippy lints are stylistic/structural choices in chart rendering, data
//   alignment loops, and test fixtures — not correctness issues.
#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::if_same_then_else)]

mod ai;
mod alerts;
mod api;
mod ask;
mod backtest;
mod benchmark;
mod bonds;
mod brokers;
mod calendar;
mod charts;
mod clock;
mod commands;
mod commodity;
mod config;
mod daemon;
mod daemon_status;
mod derivatives;
mod display;
mod funds;
mod goals;
mod greeks;
mod gsec;
mod i18n;
mod insights;
mod intraday;
mod longterm;
mod market;
#[cfg(feature = "mcp")]
mod mcp;
mod notify;
mod output;
mod pagecache;
mod payoff;
mod portfolio;
mod practice;
mod providers;
#[cfg(feature = "server")]
mod server;
mod simulator;
mod store;
mod tax;
mod tax_export;
mod technical;
#[cfg(test)]
mod testutil;
mod tools;
#[cfg(feature = "tui")]
mod tui;
mod views;
mod watchlist;
mod wealth;
mod xlsx;

use clap::parser::ValueSource;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use market::Market;
use output::Format;

/// Periods Yahoo Finance understands. Short periods give too few bars for
/// some indicators; those commands say so instead of guessing.
const PERIODS: [&str; 12] = [
    "1d", "5d", "1w", "1mo", "3mo", "6mo", "ytd", "1y", "2y", "5y", "10y", "max",
];

#[derive(Parser)]
#[command(
    name = "stockwise",
    version,
    about = "Stock market toolkit for your terminal — US and Indian markets",
    long_about = "StockWise — quotes, analysis, screeners, backtests, paper trading, and \
portfolio tracking for US and Indian (NSE/BSE) stocks, from your terminal.\n\n\
The Indian market is the default: `stockwise quote TCS` looks up TCS.NS. \
Use `-m us` (or set STOCKWISE_MARKET=us) for US stocks.\n\n\
Not financial advice. See DISCLAIMER.md.",
    after_help = "Examples:\n  \
stockwise quote RELIANCE TCS          Indian stocks (default market)\n  \
stockwise quote AAPL NVDA -m us       US stocks\n  \
stockwise deep HDFCBANK               everything about one stock\n  \
stockwise backtest INFY -s macd       test a strategy on history\n  \
stockwise quote AAPL -m us --json     machine-readable output\n  \
stockwise portfolio --csv > p.csv     spreadsheet-ready CSV\n\n\
Run `stockwise doctor` to check your setup, `stockwise config init` to set your own \
defaults, `stockwise <command> --help` for details."
)]
struct Cli {
    /// Market for bare tickers: `in` (India, adds .NS), `us`, `crypto`, `fx`, `commodity`, or an exchange: `uk`, `de`, `jp`, `hk`, `sg`, `ca`, `au`
    #[arg(
        short = 'm',
        long = "market",
        global = true,
        default_value = "in",
        env = "STOCKWISE_MARKET",
        ignore_case = true
    )]
    market: Market,

    /// Add an AI section to analysis commands (local Ollama unless STOCKWISE_AI_PROVIDER picks a cloud model)
    #[arg(
        long,
        global = true,
        env = "STOCKWISE_AI",
        value_parser = clap::builder::BoolishValueParser::new(),
        overrides_with = "no_ai"
    )]
    ai: bool,

    /// Turn the AI section off, even if STOCKWISE_AI or the config file turn it on
    #[arg(long, global = true, overrides_with = "ai")]
    no_ai: bool,

    /// Language for tables and messages: `en` or `hi` (Hindi). Default: your locale
    #[arg(
        long,
        global = true,
        env = "STOCKWISE_LANG",
        ignore_case = true,
        value_name = "LANG"
    )]
    lang: Option<i18n::Lang>,

    /// Currency bare crypto tickers are quoted in with `-m crypto` (BTC → BTC-INR)
    #[arg(
        long,
        global = true,
        env = "STOCKWISE_QUOTE_CURRENCY",
        default_value = "usd",
        value_name = "CODE",
        value_parser = parse_currency_code
    )]
    quote_currency: String,
    /// Portfolio to use: `default` (portfolio.json) or a name such as `long-term`
    #[arg(
        long,
        global = true,
        env = "STOCKWISE_PORTFOLIO",
        value_name = "NAME",
        value_parser = portfolio_name
    )]
    portfolio: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

fn portfolio_name(s: &str) -> Result<String, String> {
    portfolio::normalize_name(Some(s))
        .map(|n| n.unwrap_or_else(|| "default".into()))
        .map_err(|e| e.to_string())
}

#[derive(Subcommand)]
enum Commands {
    // ── Start here ──
    /// Real-time quote for one or more stocks
    #[command(alias = "q")]
    Quote {
        /// Tickers, e.g. RELIANCE TCS (India) or AAPL MSFT with -m us
        #[arg(required = true, num_args = 1..)]
        symbols: Vec<String>,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Everything about a stock in one command
    Deep {
        symbol: String,
        #[arg(short, long, default_value = "3mo", value_parser = PERIODS)]
        period: String,
    },

    /// Major market indices (US + India)
    Markets {
        #[command(flatten)]
        out: TableOutput,
        /// Add the largest cryptocurrencies (trade 24/7)
        #[arg(long)]
        crypto: bool,
        /// Add major currency pairs (USD/INR, EUR/USD, ...)
        #[arg(long)]
        fx: bool,
        /// Add each exchange's main index (FTSE 100, DAX, Nikkei, ...) with its session
        #[arg(long)]
        global: bool,
    },

    /// Markets + portfolio + watchlist + alerts on one screen
    #[command(alias = "d")]
    Dashboard,

    /// Find a ticker by company name
    Search {
        /// Company name or partial ticker, e.g. "tata motors"
        query: String,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Check your setup: data directory, network, cache, AI, broker
    Doctor,

    // ── Analysis ──
    /// Fundamental analysis with plain-English insights
    #[command(alias = "a")]
    Analyze { symbol: String },

    /// Technical analysis (RSI, MACD, Bollinger Bands, ...)
    #[command(alias = "t")]
    Technical {
        symbol: String,
        #[arg(short, long, default_value = "3mo", value_parser = PERIODS)]
        period: String,
        #[command(flatten)]
        out: JsonOutput,
    },

    /// Price history with a terminal chart and volume bars
    History {
        symbol: String,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
        /// Print every OHLCV bar as JSON instead of a chart
        #[arg(long, conflicts_with = "csv")]
        json: bool,
        /// Print every OHLCV bar as CSV instead of a chart
        #[arg(long)]
        csv: bool,
    },

    /// Charts for 1W, 1M, 3M and 1Y at once
    Chart { symbol: String },

    /// Compare stocks side by side
    #[command(alias = "c")]
    Compare {
        /// Two or more tickers
        #[arg(required = true, num_args = 2..)]
        symbols: Vec<String>,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Risk metrics: volatility, Sharpe, max drawdown, VaR
    Risk {
        symbol: String,
        /// History used for the metrics (daily bars; needs ~30+ trading days)
        #[arg(short, long, default_value = "1y", value_parser = PERIODS)]
        period: String,
        #[command(flatten)]
        out: JsonOutput,
    },

    /// Price correlation matrix between stocks
    #[command(alias = "corr")]
    Correlate {
        /// Two or more tickers
        #[arg(required = true, num_args = 2..)]
        symbols: Vec<String>,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Trend and momentum across timeframes at a glance
    #[command(alias = "mtf")]
    Timeframes { symbol: String },

    /// Returns over 1W, 1M, 3M, 6M, 1Y, 3Y, 5Y
    Returns {
        symbol: String,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Candlestick pattern detection
    Patterns {
        symbol: String,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },

    /// Support & resistance levels (pivot points + Fibonacci)
    Support {
        symbol: String,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },

    /// Fibonacci retracement levels
    Fibs {
        symbol: String,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },

    /// Volume profile (OBV, A/D line, volume stats)
    #[command(alias = "vol")]
    Volume {
        symbol: String,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },

    /// Detect price gaps (filled and unfilled)
    Gaps {
        symbol: String,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },

    /// Stop-loss levels (ATR, percentage, Chandelier exit)
    Stoploss {
        symbol: String,
        /// Your entry price (default: last price)
        #[arg(short, long)]
        entry: Option<f64>,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },

    /// Trend projection with confidence bands (not a prediction)
    Forecast {
        symbol: String,
        /// Days to project forward
        #[arg(short, long, default_value = "30")]
        days: usize,
        /// History used to fit the trend
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },

    // ── Market ──
    /// Top gainers and losers today
    Movers {
        /// Scan an NSE index's members instead of the built-in large caps
        #[arg(long, value_enum, ignore_case = true)]
        universe: Option<market::Universe>,
        #[command(flatten)]
        out: TableOutput,
    },
    /// Sector performance with bars
    Sectors {
        #[command(flatten)]
        out: TableOutput,
    },
    /// Market heatmap
    Heatmap {
        /// Scan an NSE index's members instead of the built-in large caps
        #[arg(long, value_enum, ignore_case = true)]
        universe: Option<market::Universe>,
    },
    /// Fear & Greed style sentiment gauge
    Sentiment,
    /// Recent news headlines for a stock
    News { symbol: String },
    /// Sector rotation — which sectors lead and lag
    Rotation,
    /// Compare two sectors head to head (e.g. IT Bank)
    Sectorcmp { sector1: String, sector2: String },
    /// Compare against industry peers
    Peers { symbol: String },
    /// Insider activity and analyst consensus
    Insider { symbol: String },
    /// IPO calendar and news
    Ipo,
    /// Earnings overview for your portfolio and watchlist
    Earnings,
    /// Earnings momentum — accelerating vs decelerating
    Surprise,
    /// Quality vs growth quadrants
    Matrix,
    /// Top-scored stocks from the large-cap universe
    Picks,
    /// Screen large caps by a preset
    Screen {
        #[arg(value_enum, ignore_case = true)]
        category: ScreenCategory,
        /// Scan an NSE index's members instead of the built-in large caps
        #[arg(long, value_enum, ignore_case = true)]
        universe: Option<market::Universe>,
        #[command(flatten)]
        out: TableOutput,
    },
    /// Dividend yield, income projection and DRIP growth
    Dividends {
        symbol: String,
        #[command(flatten)]
        out: JsonOutput,
    },

    // ── Funds & F&O ──
    /// Mutual funds (AMFI NAVs): search, nav, history, sip, screen, overlap
    Mf {
        #[command(subcommand)]
        action: MfCmd,
    },

    /// Option chains with greeks, PCR, max pain and payoffs; `options SYMBOL` shows expected moves
    #[command(
        args_conflicts_with_subcommands = true,
        subcommand_negates_reqs = true,
        after_help = "Examples:\n  \
stockwise options chain NIFTY             NSE index options, nearest expiry\n  \
stockwise options chain SENSEX            BSE index options (also BANKEX, SENSEX50, RELIANCE.BO)\n  \
stockwise options chain AAPL -m us -n 5   US options from Yahoo, 5 strikes each side\n  \
stockwise options pcr BANKNIFTY           put/call ratio and open-interest walls\n  \
stockwise options maxpain RELIANCE -e 2026-10-27\n  \
stockwise options payoff NIFTY --strategy iron-condor --width 200\n  \
stockwise options payoff NIFTY --leg \"buy call 24500\" --leg \"sell call 24800\" --today\n  \
stockwise options TCS                     expected moves (same as `options moves TCS`)"
    )]
    Options {
        #[command(subcommand)]
        action: Option<OptionsCmd>,
        /// Ticker for expected moves (same as `options moves SYMBOL`)
        #[arg(required = true)]
        symbol: Option<String>,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },

    // ── Commodities & bonds ──
    /// Gold and silver: spot in ₹/10 g and USD/oz, COMEX futures, ETFs, SGBs (default: spot)
    #[command(
        alias = "commodities",
        args_conflicts_with_subcommands = true,
        after_help = COMMODITY_HELP
    )]
    Commodity {
        #[command(subcommand)]
        action: Option<CommodityCmd>,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Indian G-secs and T-bills: benchmark yields, outstanding list, price/YTM calculator (default: yields)
    #[command(
        alias = "bond",
        args_conflicts_with_subcommands = true,
        after_help = BONDS_HELP
    )]
    Bonds {
        #[command(subcommand)]
        action: Option<BondsCmd>,
        #[command(flatten)]
        out: TableOutput,
    },

    // ── Strategies & bots ──
    /// Backtest a strategy: one stock, a portfolio, or walk-forward
    Backtest(Box<BacktestArgs>),

    /// SIP / DCA simulator — what if you invested X every month
    Sip {
        symbol: String,
        /// Amount invested each month
        amount: f64,
        #[arg(short, long, default_value = "5y", value_parser = PERIODS)]
        period: String,
        #[command(flatten)]
        out: JsonOutput,
    },

    /// What if you had invested X on a past date
    Whatif {
        symbol: String,
        /// Amount invested
        amount: f64,
        /// Date in YYYY-MM-DD format
        date: String,
        #[command(flatten)]
        out: JsonOutput,
    },

    /// Intraday trade ideas sized for your capital (signals only, no orders)
    #[command(alias = "bot")]
    Intraday {
        /// Capital to deploy
        amount: f64,
        /// Target return for the day, in percent (e.g. 1.5)
        target: f64,
        /// Add a strategy's entry/exit rule as an extra signal: a built-in, a file in your
        /// strategies folder, file:PATH.toml or wasm:PATH.wasm (as in `backtest -s`)
        #[arg(short, long)]
        strategy: Option<String>,
    },

    /// Long-term picks for a monthly SIP budget
    Longterm {
        /// Monthly SIP budget (default 10000)
        amount: Option<f64>,
    },

    /// Background worker: intraday paper-trading loop or daily long-term scan
    Daemon {
        #[arg(value_enum, ignore_case = true)]
        mode: DaemonMode,
        /// Paper capital for intraday mode (default 25000)
        amount: Option<f64>,
    },

    /// Paper-trading simulator — practise without real money
    Sim {
        #[command(subcommand)]
        action: Option<SimCmd>,
    },

    /// AI report via local Ollama: intraday, longterm, portfolio, or a ticker
    Report {
        /// `intraday`, `longterm`, `portfolio`, or a ticker such as RELIANCE
        what: String,
    },

    // ── Portfolio ──
    /// Track holdings, lots and P&L (default: show)
    #[command(alias = "p", args_conflicts_with_subcommands = true)]
    Portfolio {
        /// Currency for totals when holdings mix INR and USD (default: the market's)
        #[arg(long, global = true, value_enum, ignore_case = true)]
        base: Option<BaseCurrency>,
        #[command(subcommand)]
        action: Option<PortfolioCmd>,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Manage your watchlist (default: show)
    #[command(alias = "w", args_conflicts_with_subcommands = true)]
    Watch {
        #[command(subcommand)]
        action: Option<WatchCmd>,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Price and technical alerts (default: list)
    #[command(args_conflicts_with_subcommands = true)]
    Alert {
        #[command(subcommand)]
        action: Option<AlertCmd>,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Upcoming results, ex-dividend dates and splits for your holdings and watchlist
    #[command(after_help = CALENDAR_HELP)]
    Calendar {
        /// Symbols to look up (default: portfolio holdings and watchlist)
        symbols: Vec<String>,
        /// How many days ahead to show
        #[arg(long, default_value = "30", value_parser = clap::value_parser!(u32).range(1..=365))]
        days: u32,
        /// Only portfolio holdings (the global --portfolio NAME picks which portfolio)
        #[arg(long, conflicts_with_all = ["watchlist", "symbols"])]
        holdings: bool,
        /// Only the watchlist
        #[arg(long, conflicts_with = "symbols")]
        watchlist: bool,
        #[command(flatten)]
        out: TableOutput,
    },

    /// Export portfolio or watchlist to CSV in the current directory
    Export {
        #[arg(value_enum, ignore_case = true)]
        what: ExportWhat,
    },

    /// Import holdings from a broker CSV export or a CAS statement
    Import {
        #[arg(value_enum, ignore_case = true)]
        source: ImportSource,
        /// Path to the exported CSV file (or casparser JSON/CSV for `cas`)
        file: String,
        /// Rebuild holdings from a dated file even if that replaces lots or
        /// sales recorded another way (manual entries, another import)
        #[arg(long)]
        replace: bool,
    },

    /// Snapshot portfolio value over time (run daily)
    Wealth,
    /// Trades to rebalance your portfolio to equal weight
    Rebalance,
    /// Tax-loss harvesting opportunities
    Harvest,
    /// Capital-gains tax estimate; `tax export` writes an ITR Schedule CG / Form 8949 CSV
    #[command(
        args_conflicts_with_subcommands = true,
        subcommand_negates_reqs = true,
        after_help = "Examples:\n  \
stockwise tax in                                  this tax year's estimate\n  \
stockwise tax export in --fy 2025-26 --out cg.csv ITR Schedule CG / 112A CSV\n  \
stockwise tax export us --fy 2025 --json          Form 8949 rows as JSON"
    )]
    Tax {
        #[command(subcommand)]
        action: Option<TaxCmd>,
        #[arg(value_enum, ignore_case = true, required = true)]
        country: Option<TaxCountry>,
        #[command(flatten)]
        out: TableOutput,
    },
    /// Dividend calendar for portfolio + watchlist
    Divcal,
    /// Correlation between your holdings — find redundant positions
    Pcorr,

    // ── Planning ──
    /// Goal planner: SIP for a target corpus, retirement and education calculators
    #[command(after_help = "Examples:\n  \
stockwise goal sip --target 1cr --years 15 --step-up 10\n  \
stockwise goal retire --monthly-expense 80000 --years-to-retire 20 --years-in-retirement 30\n  \
stockwise goal education --cost 25L --years 10 --json\n\n\
Amounts take Indian units: 50k, 25L (lakh), 1cr (crore).")]
    Goal {
        #[command(subcommand)]
        action: GoalCmd,
    },

    // ── Live trading ──
    /// Live orders: Angel One; Zerodha/Upstox/Dhan/Fyers experimental (real money — try `sim` first)
    Trade {
        /// Broker to use. angel is the original integration; zerodha, upstox, dhan and
        /// fyers are experimental — untested against a live account
        #[arg(
            long,
            global = true,
            value_enum,
            env = "STOCKWISE_BROKER",
            default_value = "angel",
            ignore_case = true
        )]
        broker: brokers::BrokerKind,
        #[command(subcommand)]
        action: TradeCmd,
    },

    // ── AI assistants ──
    /// Ask in plain English; the AI model answers with StockWise's read-only tools
    #[command(after_help = "Examples:\n  \
stockwise ask \"how is my portfolio doing?\"\n  \
stockwise ask \"compare TCS and INFY on RSI and 1-year risk\" --verbose\n  \
stockwise ask \"what SIP reaches 1 crore in 15 years at 12%?\" --json\n\n\
Uses STOCKWISE_AI_PROVIDER (local Ollama by default) and needs a model with tool calling. \
The tools only read data; nothing is ever ordered. Not financial advice.")]
    Ask {
        /// The question (quote it, or give it as several words)
        #[arg(required = true, num_args = 1..)]
        question: Vec<String>,
        /// Most model requests; the last one must answer without tools
        #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u32).range(1..=20))]
        max_steps: u32,
        /// Token budget across all requests (input + output); past it the model must answer
        #[arg(long, default_value_t = 100_000, value_parser = clap::value_parser!(u64).range(1_000..=2_000_000))]
        max_tokens: u64,
        /// Show each tool call and the size of its result on stderr
        #[arg(short, long)]
        verbose: bool,
        #[command(flatten)]
        out: JsonOutput,
    },

    /// MCP server for AI assistants (Claude, ChatGPT, Cursor): read-only tools over stdio or HTTP
    #[command(after_help = "Examples:\n  \
stockwise mcp                          stdio, for Claude Desktop / Claude Code / Cursor\n  \
stockwise mcp --http 127.0.0.1:8788    Streamable HTTP at http://127.0.0.1:8788/mcp\n  \
stockwise mcp --list-tools             the tools and their input schemas as JSON\n\n\
Claude Code: claude mcp add stockwise -- stockwise mcp\n\
Tools are read-only. Live orders are never exposed: --allow-orders only adds a dry-run preview \
that returns the `stockwise trade` command for you to run yourself.")]
    Mcp {
        /// Serve Streamable HTTP at http://ADDR/mcp instead of stdio, e.g. 127.0.0.1:8788
        #[arg(long, value_name = "ADDR")]
        http: Option<String>,
        /// Allow a non-loopback --http address; requires a STOCKWISE_API_TOKEN bearer token
        #[arg(long, requires = "http")]
        allow_remote: bool,
        /// Add an order_preview tool (dry-run previews only; it never places an order)
        #[arg(long)]
        allow_orders: bool,
        /// Print the tools and their input schemas as JSON and exit
        #[arg(long)]
        list_tools: bool,
    },

    // ── Interfaces ──
    /// Full-screen live dashboard: indices, watchlist, portfolio, alerts, charts
    Tui {
        /// Seconds between price refreshes (quotes are cached for 30s)
        #[arg(
            short,
            long,
            default_value_t = 30,
            value_parser = clap::value_parser!(u64).range(10..=3600)
        )]
        interval: u64,
        /// Print one frame of COLSxROWS to stdout and exit (for screenshots)
        #[arg(long, value_name = "COLSxROWS", hide = true)]
        snapshot: Option<String>,
    },

    /// Read-only web dashboard, JSON API and Prometheus metrics over HTTP
    Serve {
        /// Address to listen on. Non-loopback needs --allow-remote
        #[arg(long, default_value = "127.0.0.1:8787", env = "STOCKWISE_BIND")]
        bind: String,
        /// Allow a non-loopback --bind; requires a STOCKWISE_API_TOKEN bearer token
        #[arg(long)]
        allow_remote: bool,
        /// Open the dashboard in your browser once the server is listening
        #[arg(long)]
        open: bool,
    },

    // ── Setup ──
    /// Your defaults: config file path, effective settings, template (default: show)
    Config {
        #[command(subcommand)]
        action: Option<ConfigCmd>,
    },

    /// Notification channels for alerts and the daemon (default: status)
    Notify {
        #[command(subcommand)]
        action: Option<NotifyCmd>,
    },

    /// Print a shell completion script (bash, zsh, fish, powershell, elvish)
    Completions {
        #[arg(value_enum, ignore_case = true)]
        shell: clap_complete::Shell,
    },
    /// Print the man page, or write one page per command with --dir
    #[command(after_help = "Examples:\n  \
stockwise man > stockwise.1 && man ./stockwise.1\n  \
stockwise man --dir ~/.local/share/man/man1    then: man stockwise-backtest")]
    Man {
        /// Write stockwise.1 and a stockwise-<command>.1 page per command into DIR
        #[arg(long, value_name = "DIR")]
        dir: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum PortfolioCmd {
    /// Add a purchase as a new lot
    Add {
        symbol: String,
        /// Number of shares
        shares: f64,
        /// Price paid per share
        cost: f64,
        /// Purchase date, YYYY-MM-DD (default: today) — used for LTCG/STCG and XIRR
        #[arg(long)]
        date: Option<String>,
    },
    /// Record a sale: oldest lots first (FIFO), realized P&L kept for `tax`
    Sell {
        symbol: String,
        /// Number of shares sold
        shares: f64,
        /// Sale price per share (default: the live price; required with a past --date)
        price: Option<f64>,
        /// Sale date, YYYY-MM-DD (default: today)
        #[arg(long)]
        date: Option<String>,
    },
    /// A holding's lots, sales and dividends; --lot N --date fixes a purchase date
    Lots {
        symbol: String,
        /// Lot number (from the list) whose purchase date to correct
        #[arg(long, requires = "date")]
        lot: Option<usize>,
        /// Correct purchase date for --lot, YYYY-MM-DD
        #[arg(long, requires = "lot")]
        date: Option<String>,
    },
    /// Splits, bonuses and dividends since you bought; --apply records them
    Actions {
        /// Only this holding (default: all)
        symbol: Option<String>,
        /// Adjust lots and record dividends (otherwise only shows them)
        #[arg(long)]
        apply: bool,
    },
    /// Remove a holding and its lots without recording a sale
    #[command(alias = "rm")]
    Remove { symbol: String },
    /// Show holdings with live P&L and XIRR
    Show {
        #[command(flatten)]
        out: TableOutput,
    },
    /// List your portfolios (see --portfolio)
    List,
    /// Time-weighted return vs NIFTY 50 / S&P 500: alpha, beta, attribution by holding and sector
    Benchmark {
        /// NIFTY, SENSEX, BANKNIFTY, NIFTY500, SP500, NASDAQ, NASDAQ100, DOW, an index such as
        /// ^NSEI, or a ticker [default: NIFTY; SP500 with -m us]
        benchmark: Option<String>,
        #[arg(short, long, default_value = "1y", value_parser = BENCHMARK_PERIODS)]
        period: String,
        #[command(flatten)]
        out: TableOutput,
    },
}

/// Periods `portfolio benchmark` accepts: long enough for daily returns.
const BENCHMARK_PERIODS: [&str; 9] = ["1mo", "3mo", "6mo", "ytd", "1y", "2y", "5y", "10y", "max"];

#[derive(Subcommand)]
enum TaxCmd {
    /// Capital-gains schedule of a tax year's sales: ITR Schedule CG / 112A (in) or Form 8949 (us)
    #[command(
        after_help = "An estimate from the sales StockWise recorded (`portfolio sell`, \
tradebook and CAS imports) — verify it with your broker's tax P&L before filing."
    )]
    Export {
        /// `in` (ITR Schedule CG / 112A) or `us` (Form 8949) [default: in; us with -m us]
        #[arg(value_enum, ignore_case = true)]
        country: Option<ExportCountry>,
        /// Tax year: 2025-26 or AY2026-27 for India, 2025 for the US [default: the current one]
        #[arg(long, value_name = "YEAR")]
        fy: Option<String>,
        /// Write the file here and print a summary, instead of printing it
        #[arg(long, value_name = "FILE")]
        out: Option<std::path::PathBuf>,
        /// JSON instead of CSV
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportCountry {
    /// India: ITR Schedule CG / Schedule 112A
    #[value(alias = "india")]
    In,
    /// United States: Form 8949 and Schedule D totals
    #[value(alias = "usa")]
    Us,
}

#[derive(Subcommand)]
enum GoalCmd {
    /// Monthly SIP needed for a target corpus, with an optional yearly step-up
    Sip {
        /// Target corpus: 1cr, 50L, 2500000, ...
        #[arg(long, value_parser = goal_amount)]
        target: f64,
        /// Years to the goal (1-60)
        #[arg(long, value_parser = goal_years)]
        years: u32,
        /// Expected annual return, in percent
        #[arg(long = "return", value_name = "PCT", default_value_t = 12.0, value_parser = return_pct)]
        return_pct: f64,
        #[command(flatten)]
        sip: GoalSipFlags,
    },
    /// Retirement corpus for today's monthly expenses, and the SIP that builds it
    Retire {
        /// Monthly expenses in today's money: 80000, 80k, 1.2L, ...
        #[arg(long, value_parser = goal_amount)]
        monthly_expense: f64,
        /// Years until you retire (1-60)
        #[arg(long, value_parser = goal_years)]
        years_to_retire: u32,
        /// Years the corpus must last (1-60)
        #[arg(long, value_parser = goal_years)]
        years_in_retirement: u32,
        /// Yearly inflation of your expenses, in percent
        #[arg(long, value_name = "PCT", default_value_t = 6.0, value_parser = inflation_pct)]
        inflation: f64,
        /// Annual return until retirement, in percent
        #[arg(long, value_name = "PCT", default_value_t = 12.0, value_parser = return_pct)]
        return_pre: f64,
        /// Annual return on the corpus during retirement, in percent
        #[arg(long, value_name = "PCT", default_value_t = 7.0, value_parser = return_pct)]
        return_post: f64,
        #[command(flatten)]
        sip: GoalSipFlags,
    },
    /// Education cost in N years, grown by education inflation, and the SIP that pays it
    Education {
        /// Cost in today's money: 25L, 40 lakh, 1.2cr, ...
        #[arg(long, value_parser = goal_amount)]
        cost: f64,
        /// Years until it is needed (1-60)
        #[arg(long, value_parser = goal_years)]
        years: u32,
        /// Yearly education inflation, in percent
        #[arg(long, value_name = "PCT", default_value_t = 8.0, value_parser = inflation_pct)]
        inflation: f64,
        /// Expected annual return, in percent
        #[arg(long = "return", value_name = "PCT", default_value_t = 12.0, value_parser = return_pct)]
        return_pct: f64,
        #[command(flatten)]
        sip: GoalSipFlags,
    },
}

/// SIP options shared by every goal.
#[derive(Args, Clone, Copy)]
struct GoalSipFlags {
    /// Raise the SIP by this percent every year
    #[arg(long, value_name = "PCT", default_value_t = 0.0, value_parser = step_up_pct)]
    step_up: f64,
    /// Money already invested towards the goal (grows at the same return)
    #[arg(long, value_parser = goal_amount)]
    savings: Option<f64>,
    #[command(flatten)]
    out: JsonOutput,
}

fn goal_amount(s: &str) -> Result<f64, String> {
    goals::parse_amount(s)
}

fn goal_years(s: &str) -> Result<u32, String> {
    match s.trim().parse::<u32>() {
        Ok(y) if (1..=60).contains(&y) => Ok(y),
        _ => Err(format!(
            "`{s}` must be a whole number of years from 1 to 60"
        )),
    }
}

fn percent_between(s: &str, lo: f64, hi: f64) -> Result<f64, String> {
    match s.trim().trim_end_matches('%').parse::<f64>() {
        Ok(v) if (lo..=hi).contains(&v) => Ok(v),
        _ => Err(format!("`{s}` must be a percentage from {lo} to {hi}")),
    }
}

fn return_pct(s: &str) -> Result<f64, String> {
    percent_between(s, 0.0, 50.0)
}

fn inflation_pct(s: &str) -> Result<f64, String> {
    percent_between(s, 0.0, 30.0)
}

fn step_up_pct(s: &str) -> Result<f64, String> {
    percent_between(s, 0.0, 50.0)
}

#[derive(Clone, Copy, ValueEnum)]
enum BaseCurrency {
    /// Indian rupee
    Inr,
    /// US dollar
    Usd,
}

#[derive(Subcommand)]
enum WatchCmd {
    /// Add tickers to the watchlist
    Add {
        #[arg(required = true, num_args = 1..)]
        symbols: Vec<String>,
    },
    /// Remove tickers from the watchlist
    #[command(alias = "rm")]
    Remove {
        #[arg(required = true, num_args = 1..)]
        symbols: Vec<String>,
    },
    /// Show the watchlist with live prices
    #[command(alias = "list")]
    Show {
        #[command(flatten)]
        out: TableOutput,
    },
}

const ALERT_ADD_HELP: &str = "\
Conditions (NUMBER is what follows the condition):
  above / below PRICE         live price at or beyond a level
  rsi-below / rsi-above LEVEL RSI (14-day, or --period) at or beyond LEVEL
  cross-above-sma / cross-below-sma DAYS
  cross-above-ema / cross-below-ema DAYS
                              daily close crosses the moving average (fires on
                              the crossing day only)
  breakout-high / breakdown-low DAYS
                              close beyond the previous DAYS-day high / low
  change-above / change-below PCT
                              day change of +PCT% or more / -PCT% or less
  volume-spike MULTIPLE       volume at least MULTIPLE x its 20-day (--period)
                              average
  results-in DAYS             results due within DAYS days (see `calendar`)

Technical conditions use daily bars, updated with the live price.

Examples:
  stockwise alert add TCS above 4200
  stockwise alert add INFY rsi-below 30
  stockwise alert add RELIANCE cross-above-sma 200
  stockwise alert add AAPL -m us breakout-high 52
  stockwise alert add HDFCBANK change-below 3
  stockwise alert add TCS results-in 7";

const CALENDAR_HELP: &str = "\
Results and earnings dates, ex-dividend and payment dates come from Yahoo
Finance. For Indian stocks, NSE board meetings and corporate actions add filed
results dates, dividends, splits and bonus issues. Answers are cached for a day.

Examples:
  stockwise calendar
  stockwise calendar --holdings --days 14
  stockwise calendar TCS INFY
  stockwise calendar AAPL MSFT -m us --json";

#[derive(Subcommand)]
enum AlertCmd {
    /// Alert on a price level or a technical condition, e.g. `alert add TCS rsi-below 30`
    #[command(after_help = ALERT_ADD_HELP)]
    Add {
        symbol: String,
        #[arg(value_enum, ignore_case = true)]
        condition: AlertWhen,
        /// The condition's number: a price, RSI level, days, percent or multiple
        #[arg(allow_negative_numbers = true)]
        target: f64,
        /// RSI period (default 14) or volume-average length (default 20)
        #[arg(long, value_parser = clap::value_parser!(u32).range(2..=200))]
        period: Option<u32>,
    },
    /// Remove an alert by its number from `alert list`
    #[command(alias = "rm")]
    Remove { index: usize },
    /// Check all alerts against live prices
    Check {
        #[command(flatten)]
        out: TableOutput,
    },
    /// Keep checking alerts while their market is open; notify each trigger once
    Watch {
        /// Seconds between checks
        #[arg(long, default_value = "60", value_parser = clap::value_parser!(u64).range(10..))]
        interval: u64,
        /// Check every alert once (any time of day) and exit — for cron
        #[arg(long)]
        once: bool,
    },
    /// List alerts
    #[command(alias = "show")]
    List {
        #[command(flatten)]
        out: TableOutput,
    },
}

#[derive(Subcommand)]
enum SimCmd {
    /// Start a paper session with the bot's picks
    Start {
        /// Paper capital (default 25000)
        amount: Option<f64>,
        /// Target return in percent
        target: Option<f64>,
    },
    /// Live P&L of the open session
    Status,
    /// Settle open sessions from each day's 5-minute bars (stop or target first?)
    Settle {
        /// Settle today's session before the close, at the latest bar
        #[arg(long)]
        now: bool,
    },
    /// Past sessions and the bot's confidence rating
    #[command(alias = "stats")]
    History {
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Delete all simulator history (a backup is kept)
    Reset {
        /// Don't ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Historical replay: trade a past market day by day, the future hidden
    #[command(after_help = REPLAY_HELP)]
    Replay {
        #[command(subcommand)]
        action: ReplayCmd,
    },
    /// Goal-based challenges over a replay or live paper sessions
    Challenge {
        #[command(subcommand)]
        action: ChallengeCmd,
    },
    /// Your badges, streaks and best scores
    Profile {
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Shareable card of your last finished run (text, or SVG with --svg)
    Card {
        /// Write the card as an SVG image to FILE
        #[arg(long, value_name = "FILE")]
        svg: Option<std::path::PathBuf>,
        /// Also print the text card (the default without --svg)
        #[arg(long)]
        text: bool,
    },
}

const REPLAY_HELP: &str = "\
Examples:
  stockwise sim replay list
  stockwise sim replay start covid-crash-2020
  stockwise sim replay buy RELIANCE 20 --stop 1250 --target 1600
  stockwise sim replay next 5
  stockwise sim replay chart index
  stockwise sim replay end
  stockwise -m us sim replay start --from 2022-01-03 --to 2022-06-30 --symbols AAPL,MSFT,XOM

Prices come from daily history fetched once at the start; you only ever see bars up
to the replay date. Orders fill at the next day's open with the backtest cost models.
Paper money only — not financial advice. Guide: docs/simulator.md";

#[derive(Subcommand)]
enum ReplayCmd {
    /// The built-in scenarios
    #[command(alias = "scenarios")]
    List {
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Start a replay: a scenario name, or --from/--to/--symbols
    Start {
        /// covid-crash-2020, gfc-2008, adani-2023, dotcom-2000, bull-run-2021, rate-hikes-2022, or file:PATH.toml
        #[arg(
            required_unless_present_all = ["from", "to"],
            conflicts_with_all = ["from", "to", "symbols", "benchmark"]
        )]
        scenario: Option<String>,
        /// First day of a custom replay (YYYY-MM-DD)
        #[arg(long, value_parser = replay_date, requires = "to")]
        from: Option<chrono::NaiveDate>,
        /// Last day of a custom replay (YYYY-MM-DD, in the past)
        #[arg(long, value_parser = replay_date, requires = "from")]
        to: Option<chrono::NaiveDate>,
        /// Stocks for a custom replay, e.g. TCS,INFY,HDFCBANK
        #[arg(long, value_delimiter = ',', num_args = 1..)]
        symbols: Vec<String>,
        /// Benchmark of a custom replay [default: ^NSEI, or ^GSPC with -m us]
        #[arg(long)]
        benchmark: Option<String>,
        /// Paper capital [default: the scenario's, 100000]
        #[arg(long, value_parser = capital)]
        capital: Option<f64>,
        /// Trading costs; auto = India delivery for INR stocks, US otherwise
        #[arg(long, default_value = "auto", value_parser = ["auto", "none", "in-delivery", "in-intraday", "us"])]
        costs: String,
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Move forward one or more trading days
    Next {
        /// Trading days to move [default: 1]
        #[arg(default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=1000))]
        days: u32,
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Queue a buy for the next day's open
    Buy {
        symbol: String,
        #[arg(value_parser = clap::value_parser!(u64).range(1..))]
        qty: u64,
        /// Stop-loss price: sell if the price falls to it
        #[arg(long, value_parser = level)]
        stop: Option<f64>,
        /// Target price: sell if the price rises to it
        #[arg(long, value_parser = level)]
        target: Option<f64>,
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Queue a sell for the next day's open [default: the whole position]
    Sell {
        symbol: String,
        #[arg(value_parser = clap::value_parser!(u64).range(1..))]
        qty: Option<u64>,
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Cancel a pending order [default: all of them]
    Cancel {
        /// Order number from `sim replay status`
        id: Option<u32>,
    },
    /// Change a position's stop-loss or target
    Set {
        symbol: String,
        #[arg(long, value_parser = level)]
        stop: Option<f64>,
        #[arg(long, value_parser = level)]
        target: Option<f64>,
    },
    /// Account, positions, orders and the market on the replay date
    Status {
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Price chart up to the replay date: a stock, or `index`
    Chart {
        symbol: String,
        /// Trading days to show
        #[arg(long, default_value_t = 60)]
        days: usize,
    },
    /// Finish the replay: scorecard, badges and what happened next
    End {
        #[command(flatten)]
        out: JsonOutput,
    },
}

#[derive(Subcommand)]
enum ChallengeCmd {
    /// The built-in challenges
    List {
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Start a challenge: inside the running replay, or over live paper sessions
    Start {
        /// beat-the-index, steady-hands, position-limit, stop-loss, sip-discipline, crash-survivor, or file:PATH.toml
        name: String,
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Progress, rules and anything broken so far
    Status {
        #[command(flatten)]
        out: JsonOutput,
    },
    /// End the challenge now and see the result
    End {
        #[command(flatten)]
        out: JsonOutput,
    },
}

fn replay_date(s: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|_| format!("`{s}`: use YYYY-MM-DD, e.g. 2020-02-03"))
}

/// A stop-loss or target price.
fn level(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => Ok(v),
        _ => Err(format!("`{s}` must be a price above 0")),
    }
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the config file path (it may not exist yet)
    Path,
    /// Effective settings and where each comes from: flag, env, config file, or default
    Show {
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Write a commented config template to the config file path
    Init {
        /// Replace an existing config file
        #[arg(long)]
        force: bool,
    },
}

/// `--json` / `--csv` for commands that print a table.
#[derive(Args, Clone, Copy, Default)]
struct TableOutput {
    /// Print JSON instead of a table
    #[arg(long, conflicts_with = "csv")]
    json: bool,
    /// Print CSV (RFC 4180, header row first) instead of a table
    #[arg(long)]
    csv: bool,
}

/// `--json` for commands whose output is more than one table.
#[derive(Args, Clone, Copy, Default)]
struct JsonOutput {
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

impl From<TableOutput> for Format {
    fn from(o: TableOutput) -> Self {
        Format::from_flags(o.json, o.csv)
    }
}

impl From<JsonOutput> for Format {
    fn from(o: JsonOutput) -> Self {
        Format::from_flags(o.json, false)
    }
}

impl TableOutput {
    /// Flags given on either the parent (`portfolio --json`) or the action
    /// (`portfolio show --json`).
    fn or(self, other: TableOutput) -> Format {
        Format::from_flags(self.json || other.json, self.csv || other.csv)
    }
}

/// Months for an MF period such as `6mo`, `3y` or `max` (validated here,
/// converted again in the handler).
fn mf_period(s: &str) -> Result<String, String> {
    funds::parse_period(s).map(|_| s.trim().to_lowercase())
}

const COMMODITY_HELP: &str = "\
Spot is the COMEX front-month future (Yahoo GC=F / SI=F) converted at USD/INR:
international prices, before Indian import duty and GST. MCX futures have no
free public feed. With -m commodity, GOLD and SILVER resolve to GC=F and SI=F
and other bare tickers to NSE (GOLDBEES → GOLDBEES.NS), so history, chart,
technical and alerts work on them too.

Examples:
  stockwise commodity                       gold and silver in ₹/g, ₹/10 g, ₹/kg and USD/oz
  stockwise commodity sgb                   Sovereign Gold Bonds vs gold spot
  stockwise commodity etfs --csv            gold and silver ETFs on NSE
  stockwise commodity futures --json        COMEX gold and silver contracts
  stockwise history gold -m commodity -p 1y
  stockwise alert add silver above 60 -m commodity";

const BONDS_HELP: &str = "\
Yields come from CCIL's tenor-wise indicative yields (RBI's benchmarks when
CCIL is unavailable), cached for 6 hours. The outstanding list is RBI's.
Neither publishes prices of individual G-secs, and NSE's live debt quotes have
no documented API, so StockWise does not show bond prices: enter the price
your broker shows in `bonds calc`.

Examples:
  stockwise bonds                                 benchmark yields, incl. India 10Y
  stockwise bonds list 2033                       G-secs maturing in 2033
  stockwise bonds calc 718GS2033 --price 101.25   yield to maturity
  stockwise bonds calc \"6.94% GS 2036\" --ytm 7.4  price for a yield
  stockwise bonds calc --coupon 7.1 --maturity 2034-04-08 --price 99.5
  stockwise bonds calc --tbill --maturity 2027-01-14 --price 98.6";

#[derive(Subcommand)]
enum CommodityCmd {
    /// Gold and silver spot: USD per troy ounce and ₹ per gram, 10 g and kg (before duty and GST)
    Spot {
        #[command(flatten)]
        out: TableOutput,
    },
    /// Gold and silver futures — COMEX via Yahoo (MCX has no public feed)
    Futures {
        #[command(flatten)]
        out: TableOutput,
    },
    /// Gold and silver ETFs listed on NSE (GOLDBEES, SILVERBEES, ...)
    Etfs {
        #[command(flatten)]
        out: TableOutput,
    },
    /// Sovereign Gold Bonds on NSE: price vs gold spot, maturity, 2.5% interest
    Sgb {
        #[command(flatten)]
        out: TableOutput,
    },
}

#[derive(Subcommand)]
enum BondsCmd {
    /// Benchmark yields by tenor: T-bills, G-secs (India 10Y) and SDLs
    Yields {
        #[command(flatten)]
        out: TableOutput,
    },
    /// Government of India securities outstanding (RBI); QUERY = maturity year or name
    List {
        /// A maturity year (2033), part of a name ("GS 2036"), an ISIN or an NSE symbol
        query: Option<String>,
        #[command(flatten)]
        out: TableOutput,
    },
    /// Price ↔ yield to maturity, accrued interest and duration for a G-sec or T-bill
    #[command(group(clap::ArgGroup::new("input").required(true).args(["price", "ytm"])))]
    Calc {
        /// The security: NSE symbol (718GS2033), name ("7.18% GS 2033") or ISIN, looked up in RBI's list
        #[arg(required_unless_present = "maturity")]
        security: Option<String>,
        /// Annual coupon in percent (instead of looking the security up)
        #[arg(long, value_parser = coupon_pct)]
        coupon: Option<f64>,
        /// Maturity date, YYYY-MM-DD
        #[arg(long, value_parser = expiry_date)]
        maturity: Option<chrono::NaiveDate>,
        /// Price per ₹100 face value (clean, unless --dirty)
        #[arg(long, value_parser = bond_price)]
        price: Option<f64>,
        /// Yield to maturity in percent, to get the price
        #[arg(long, allow_negative_numbers = true)]
        ytm: Option<f64>,
        /// Settlement date, YYYY-MM-DD (default: today in India)
        #[arg(long, value_parser = expiry_date)]
        settle: Option<chrono::NaiveDate>,
        /// A Treasury bill: discount yield, no coupon
        #[arg(long, conflicts_with = "coupon")]
        tbill: bool,
        /// The --price includes accrued interest (NSE quotes G-secs dirty)
        #[arg(long, conflicts_with = "tbill")]
        dirty: bool,
        #[command(flatten)]
        out: JsonOutput,
    },
}

fn coupon_pct(s: &str) -> Result<f64, String> {
    match s.trim().trim_end_matches('%').parse::<f64>() {
        Ok(v) if (0.0..=50.0).contains(&v) => Ok(v),
        _ => Err(format!("`{s}`: a coupon in percent, e.g. 7.18")),
    }
}

fn bond_price(s: &str) -> Result<f64, String> {
    match s.trim().parse::<f64>() {
        Ok(v) if v > 0.0 && v.is_finite() => Ok(v),
        _ => Err(format!("`{s}`: a price per ₹100 face value, e.g. 101.25")),
    }
}

fn expiry_date(s: &str) -> Result<chrono::NaiveDate, String> {
    derivatives::parse_expiry(s).ok_or_else(|| format!("`{s}`: use YYYY-MM-DD, e.g. 2026-10-27"))
}

#[derive(Subcommand)]
enum MfCmd {
    /// Find schemes by name, fund house or category, e.g. `mf search parag flexi direct`
    Search {
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Maximum schemes to show
        #[arg(short = 'n', long, default_value_t = 25)]
        limit: usize,
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Latest NAV from AMFI for one or more scheme codes
    Nav {
        /// AMFI scheme codes (from `mf search`)
        #[arg(required = true, num_args = 1..)]
        codes: Vec<u32>,
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// NAV chart, 1M–10Y trailing returns (CAGR from 1Y) and drawdown
    History {
        /// AMFI scheme code
        code: u32,
        /// Chart period: 6mo, 1y, 3y, 5y, 10y, max, ...
        #[arg(short, long, default_value = "1y", value_parser = mf_period)]
        period: String,
        /// Print JSON (returns and every NAV in the period)
        #[arg(long)]
        json: bool,
    },
    /// Monthly SIP into a scheme from NAV history, with XIRR
    Sip {
        /// AMFI scheme code
        code: u32,
        /// Amount invested each month (₹)
        amount: f64,
        /// How far back the SIP starts: 1y, 3y, 5y, 10y, max, ...
        #[arg(short, long, default_value = "5y", value_parser = mf_period)]
        period: String,
        /// Print JSON instead of a chart
        #[arg(long)]
        json: bool,
    },
    /// Rank a category's growth funds by 1Y/3Y/5Y return or expense ratio (bare: list categories)
    Screen {
        /// Category words, e.g. `flexi cap`, `elss`, `liquid`, `equity etf`
        category: Vec<String>,
        /// Which plans to include
        #[arg(long, value_enum, ignore_case = true, default_value = "direct")]
        plan: MfPlan,
        /// Rank by return horizon, or by expense ratio (`ter`, lowest first)
        #[arg(short, long, value_enum, default_value = "3y")]
        sort: MfSort,
        /// Only funds whose disclosed TER is at most this many percent, e.g. 1.0
        #[arg(long, value_name = "PERCENT", value_parser = max_ter)]
        max_ter: Option<f64>,
        /// Funds to show
        #[arg(short = 'n', long, default_value_t = 15)]
        limit: usize,
        /// Most funds to fetch history for (one request each, cached 12h)
        #[arg(long, default_value_t = 100)]
        max: usize,
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Holdings and sector overlap of two funds/ETFs (Yahoo data: mainly US funds)
    Overlap {
        /// First fund or ETF ticker, e.g. SPY (with -m us)
        a: String,
        /// Second fund or ETF ticker, e.g. QQQ
        b: String,
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum MfPlan {
    /// Direct plans (lower expense ratio)
    Direct,
    /// Regular plans (sold through distributors)
    Regular,
    /// Both
    All,
}

#[derive(Clone, Copy, ValueEnum)]
enum MfSort {
    #[value(name = "1y")]
    Y1,
    #[value(name = "3y")]
    Y3,
    #[value(name = "5y")]
    Y5,
    /// Total expense ratio, lowest first
    Ter,
}

fn max_ter(s: &str) -> Result<f64, String> {
    match s.trim().trim_end_matches('%').parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 && v <= 5.0 => Ok(v),
        _ => Err(format!(
            "`{s}`: a percentage above 0 and at most 5, e.g. 1.0"
        )),
    }
}

#[derive(Subcommand)]
enum OptionsCmd {
    /// Option chain: OI, volume, IV and greeks (NSE for India, Yahoo for -m us)
    Chain {
        /// NIFTY, BANKNIFTY, an F&O stock such as RELIANCE, or a US ticker with -m us
        symbol: String,
        /// Expiry date (default: nearest)
        #[arg(short, long, value_parser = expiry_date)]
        expiry: Option<chrono::NaiveDate>,
        /// Strikes each side of at-the-money (0 = all)
        #[arg(short = 'n', long, default_value_t = 10)]
        strikes: usize,
        /// Risk-free rate in percent for greeks (default 6.5 India, 4.0 US)
        #[arg(long)]
        rate: Option<f64>,
        /// Dividend yield in percent for greeks
        #[arg(long, default_value_t = 0.0)]
        div_yield: f64,
        /// Print JSON instead of tables
        #[arg(long)]
        json: bool,
    },
    /// Put/call ratio by open interest and volume, with the biggest OI strikes
    Pcr {
        symbol: String,
        /// Expiry date (default: nearest)
        #[arg(short, long, value_parser = expiry_date)]
        expiry: Option<chrono::NaiveDate>,
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Max pain: the expiry price at which option buyers collect least
    #[command(alias = "max-pain")]
    Maxpain {
        symbol: String,
        /// Expiry date (default: nearest)
        #[arg(short, long, value_parser = expiry_date)]
        expiry: Option<chrono::NaiveDate>,
        /// Print JSON instead of a chart
        #[arg(long)]
        json: bool,
    },
    /// Payoff of a multi-leg position priced from the live chain: P/L chart,
    /// max profit/loss, breakevens, probability of profit, net greeks (no orders)
    #[command(
        after_help = "Legs: `buy|sell [QTY] call|put STRIKE [@PRICE]` or `buy|sell [QTY] stock [@PRICE]`.\n\
STRIKE is a listed strike or atm, atm+N, atm-N (N strikes from the money). Without @PRICE a\n\
leg is priced at the bid/ask mid, else the last trade (flagged `ltp`).\n\n\
Presets (around the at-the-money strike, W = --width, default two strike steps):\n  \
bull-call     buy call ATM, sell call ATM+W\n  \
bear-put      buy put ATM, sell put ATM-W\n  \
straddle      buy call and put ATM\n  \
strangle      buy put ATM-W, buy call ATM+W\n  \
iron-condor   sell put ATM-W and call ATM+W, buy put ATM-2W and call ATM+2W\n  \
butterfly     buy calls ATM-W and ATM+W, sell 2 calls ATM\n  \
covered-call  buy the underlying at spot, sell call ATM+W"
    )]
    Payoff {
        /// NIFTY, BANKNIFTY, SENSEX, an F&O stock, or a US ticker with -m us
        symbol: String,
        /// A leg, e.g. "buy call 24500" (repeat for each leg)
        #[arg(long = "leg", value_name = "LEG", value_parser = payoff::parse_leg)]
        legs: Vec<payoff::LegSpec>,
        /// A ready-made strategy around the at-the-money strike
        #[arg(short, long, value_enum)]
        strategy: Option<StrategyArg>,
        /// Strike distance for --strategy, in points (default: two strike steps)
        #[arg(short, long)]
        width: Option<f64>,
        /// Expiry date (default: nearest)
        #[arg(short, long, value_parser = expiry_date)]
        expiry: Option<chrono::NaiveDate>,
        /// Number of lots (each leg's quantity is multiplied by this)
        #[arg(short, long, default_value_t = 1)]
        lots: u32,
        /// Units per lot, when the chain does not show it (e.g. BSE after hours)
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        lot_size: Option<u32>,
        /// Risk-free rate in percent (default 6.5 India, 4.0 US)
        #[arg(long)]
        rate: Option<f64>,
        /// Dividend yield in percent
        #[arg(long, default_value_t = 0.0)]
        div_yield: f64,
        /// Volatility in percent for the probability of profit (default: ATM IV)
        #[arg(long)]
        iv: Option<f64>,
        /// Also chart today's P/L (Black-Scholes at each leg's IV)
        #[arg(long)]
        today: bool,
        /// Print JSON instead of a chart
        #[arg(long)]
        json: bool,
    },
    /// Expected moves and strike ideas from realized volatility
    Moves {
        symbol: String,
        #[arg(short, long, default_value = "1mo", value_parser = PERIODS)]
        period: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum StrategyArg {
    /// Buy ATM call, sell call ATM+W
    BullCall,
    /// Buy ATM put, sell put ATM-W
    BearPut,
    /// Buy ATM call and put
    Straddle,
    /// Buy put ATM-W and call ATM+W
    Strangle,
    /// Short iron condor: credit between ATM±W, wings at ATM±2W
    IronCondor,
    /// Long call butterfly ATM-W / 2×ATM / ATM+W
    Butterfly,
    /// Underlying at spot plus a short call ATM+W
    CoveredCall,
}

impl From<StrategyArg> for payoff::Preset {
    fn from(s: StrategyArg) -> Self {
        match s {
            StrategyArg::BullCall => payoff::Preset::BullCall,
            StrategyArg::BearPut => payoff::Preset::BearPut,
            StrategyArg::Straddle => payoff::Preset::Straddle,
            StrategyArg::Strangle => payoff::Preset::Strangle,
            StrategyArg::IronCondor => payoff::Preset::IronCondor,
            StrategyArg::Butterfly => payoff::Preset::Butterfly,
            StrategyArg::CoveredCall => payoff::Preset::CoveredCall,
        }
    }
}

#[derive(Subcommand)]
enum NotifyCmd {
    /// Show which channels are configured
    #[command(alias = "show")]
    Status,
    /// Send a test message to every configured channel
    Test,
}

#[derive(Args, Clone, Copy)]
struct OrderFlags {
    /// Place the order without the interactive confirmation
    #[arg(short, long)]
    yes: bool,
    /// Resolve and preview the order, but don't send it
    #[arg(long)]
    dry_run: bool,
}

impl From<OrderFlags> for commands::OrderOpts {
    fn from(f: OrderFlags) -> Self {
        Self {
            yes: f.yes,
            dry_run: f.dry_run,
        }
    }
}

/// Order-type prices for `trade buy` / `trade sell`: none = MARKET,
/// `--limit` = LIMIT, `--trigger` = SL-M, both = SL.
#[derive(Args, Clone, Copy)]
struct PriceFlags {
    /// Limit price (a LIMIT order; with --trigger, an SL order)
    #[arg(long)]
    limit: Option<f64>,
    /// Stop-loss trigger price (an SL-M order; with --limit, an SL order)
    #[arg(long, alias = "stop")]
    trigger: Option<f64>,
}

#[derive(Subcommand)]
enum TradeCmd {
    /// How to connect your broker account
    Setup,
    /// Create the credentials file, or show whether it is configured
    Config {
        /// Move secrets from the file into the OS keyring now
        #[arg(long)]
        keyring: bool,
    },
    /// Log in and save today's session (Zerodha, Upstox, Fyers; paste a Dhan token)
    Login,
    /// Live holdings (also synced into `portfolio`)
    Holdings,
    /// Open intraday positions
    Positions,
    /// Today's order book
    Orders,
    /// BUY on NSE: market, or --limit / --trigger for LIMIT, SL, SL-M (asks for confirmation)
    Buy {
        symbol: String,
        /// Number of shares
        qty: u32,
        #[command(flatten)]
        prices: PriceFlags,
        #[command(flatten)]
        flags: OrderFlags,
    },
    /// SELL on NSE: market, or --limit / --trigger for LIMIT, SL, SL-M (asks for confirmation)
    Sell {
        symbol: String,
        /// Number of shares
        qty: u32,
        #[command(flatten)]
        prices: PriceFlags,
        #[command(flatten)]
        flags: OrderFlags,
    },
    /// Limit BUY on NSE (asks for confirmation)
    Limit {
        symbol: String,
        /// Number of shares
        qty: u32,
        /// Limit price
        price: f64,
        #[command(flatten)]
        flags: OrderFlags,
    },
    /// GTT (good-till-triggered) orders: buy, sell, list, cancel
    Gtt {
        #[command(subcommand)]
        action: GttCmd,
    },
    /// Read-only check of the broker integration against your account; prints a
    /// redacted report to paste into a GitHub issue (never places an order)
    Selftest {
        /// Hide prices and order values in the report
        #[arg(long)]
        redact_amounts: bool,
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Pull today's executed trades (and GTT status) into the portfolio: preview, then --apply
    Sync {
        /// Ignore fills before this date (YYYY-MM-DD); Dhan also fetches its trade history
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Write the changes into the portfolio (default: preview only)
        #[arg(long)]
        apply: bool,
        /// Notify (Telegram/Slack/Discord/email) about GTT changes and synced orders
        #[arg(long)]
        notify: bool,
        #[command(flatten)]
        out: JsonOutput,
    },
}

#[derive(Subcommand)]
enum GttCmd {
    /// GTT BUY: place a LIMIT buy once the trigger trades (asks for confirmation)
    Buy {
        symbol: String,
        /// Number of shares
        qty: u32,
        /// Price that activates the GTT
        #[arg(long)]
        trigger: f64,
        /// Limit price of the order placed when it triggers
        #[arg(long)]
        limit: f64,
        #[command(flatten)]
        flags: OrderFlags,
    },
    /// GTT SELL: a stop-loss below the market or a target above it (asks for confirmation)
    Sell {
        symbol: String,
        /// Number of shares
        qty: u32,
        /// Price that activates the GTT
        #[arg(long)]
        trigger: f64,
        /// Limit price of the order placed when it triggers
        #[arg(long)]
        limit: f64,
        #[command(flatten)]
        flags: OrderFlags,
    },
    /// Your GTT orders, with status changes since the last look
    #[command(alias = "show")]
    List {
        /// Notify (Telegram/Slack/Discord/email) about GTTs that triggered or were cancelled
        #[arg(long)]
        notify: bool,
        #[command(flatten)]
        out: JsonOutput,
    },
    /// Cancel a GTT by id (asks for confirmation)
    Cancel {
        /// GTT id from `trade gtt list`
        id: String,
        #[command(flatten)]
        flags: OrderFlags,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ScreenCategory {
    /// P/E below 15
    Undervalued,
    /// Forward EPS above trailing EPS
    Growth,
    /// Dividend yield above 2%
    Dividend,
    /// Top daily gainers
    Momentum,
    /// Market cap above $100B / ₹5L Cr
    Bluechip,
}

/// `backtest SYMBOL` runs one stock; the subcommands run the other forms.
#[derive(Args)]
#[command(
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    after_help = "Examples:\n  \
stockwise backtest INFY -s macd                       one stock, Indian costs, next-open fills\n  \
stockwise backtest TCS -s rsi -P oversold=25 --json   override a parameter, JSON output\n  \
stockwise backtest portfolio TCS INFY HDFCBANK -s sma --rebalance monthly\n  \
stockwise backtest walkforward RELIANCE -s rsi --train 1y --test 3mo\n  \
stockwise backtest strategies                         built-in and your own strategies\n\n\
Strategy files, costs and fills: docs/backtesting.md"
)]
struct BacktestArgs {
    #[command(subcommand)]
    action: Option<BacktestCmd>,
    /// Ticker, e.g. INFY (India) or AAPL with -m us
    #[arg(required = true)]
    symbol: Option<String>,
    #[arg(short, long, default_value = "2y", value_parser = PERIODS)]
    period: String,
    #[command(flatten)]
    flags: BacktestFlags,
}

#[derive(Subcommand)]
enum BacktestCmd {
    /// One strategy across several stocks sharing one pot of capital
    Portfolio {
        /// Tickers, e.g. TCS INFY HDFCBANK
        #[arg(required = true, num_args = 1..)]
        symbols: Vec<String>,
        /// Capital split: equal, inverse-vol, or one weight per symbol (0.5,0.3,0.2)
        #[arg(long, default_value = "equal", value_parser = backtest::engine::Weights::parse)]
        weights: backtest::engine::Weights,
        /// Bring every symbol back to its weight each month or quarter
        #[arg(long, value_enum, ignore_case = true, default_value = "none")]
        rebalance: RebalanceArg,
        #[arg(short, long, default_value = "2y", value_parser = PERIODS)]
        period: String,
        #[command(flatten)]
        flags: BacktestFlags,
    },
    /// Tune parameters on rolling train windows; report only the unseen test windows
    #[command(alias = "wf")]
    Walkforward {
        /// Ticker, e.g. RELIANCE
        symbol: String,
        /// Train window, in trading time: 6mo, 1y, 2y, 90d
        #[arg(long, default_value = "1y", value_parser = backtest::walkforward::Window::parse)]
        train: backtest::walkforward::Window,
        /// Test window after each train window: 1mo, 3mo, 6mo
        #[arg(long, default_value = "3mo", value_parser = backtest::walkforward::Window::parse)]
        test: backtest::walkforward::Window,
        #[arg(short, long, default_value = "5y", value_parser = PERIODS)]
        period: String,
        #[command(flatten)]
        flags: BacktestFlags,
    },
    /// List built-in and your own strategies, or check strategy files for errors
    Strategies {
        /// Strategy files to check instead of listing everything
        files: Vec<String>,
        /// Print JSON instead of a list
        #[arg(long)]
        json: bool,
    },
}

/// Options shared by every form of `backtest`.
#[derive(Args, Clone)]
struct BacktestFlags {
    /// Strategy: rsi, macd, sma, bb, vwap, mr, a file in your strategies folder, file:PATH.toml,
    /// or wasm:PATH.wasm (a plugin; needs the wasm-plugins feature)
    #[arg(short, long, default_value = "rsi")]
    strategy: String,
    /// Set a strategy parameter, e.g. -P oversold=25 (repeatable)
    #[arg(
        short = 'P',
        long = "param",
        value_name = "NAME=VALUE",
        value_parser = backtest::rules::parse_param_override
    )]
    params: Vec<(String, f64)>,
    /// Trading costs; auto = India delivery for INR stocks, crypto/FX for coins and pairs, US otherwise
    #[arg(long, value_enum, ignore_case = true, default_value = "auto")]
    costs: CostsArg,
    /// Brokerage per order: 20 (flat), 0.03% or 20,0.03% (whichever is lower)
    #[arg(long, value_parser = backtest::costs::Brokerage::parse)]
    brokerage: Option<backtest::costs::Brokerage>,
    /// Slippage per fill, in basis points, 0-1000 [default: 5, or 0 with --costs none]
    #[arg(long, value_name = "BPS", value_parser = slippage_bps)]
    slippage_bps: Option<f64>,
    /// When a signal fills: the next bar's open, or the signal bar's close (look-ahead)
    #[arg(long, value_enum, ignore_case = true, default_value = "next-open")]
    fill: FillArg,
    /// Starting capital, in the stock's currency
    #[arg(long, default_value = "100000", value_parser = capital)]
    capital: f64,
    /// `--json` prints the whole result, `--csv` the trades
    #[command(flatten)]
    out: TableOutput,
}

impl From<BacktestFlags> for commands::BacktestOpts {
    fn from(f: BacktestFlags) -> Self {
        use backtest::costs::CostPreset;
        Self {
            strategy: f.strategy,
            params: f.params,
            costs: match f.costs {
                CostsArg::Auto => CostPreset::Auto,
                CostsArg::None => CostPreset::None,
                CostsArg::InDelivery => CostPreset::InDelivery,
                CostsArg::InIntraday => CostPreset::InIntraday,
                CostsArg::Us => CostPreset::Us,
                CostsArg::Crypto => CostPreset::Crypto,
                CostsArg::Fx => CostPreset::Fx,
            },
            brokerage: f.brokerage,
            slippage_bps: f.slippage_bps,
            fill: match f.fill {
                FillArg::NextOpen => backtest::engine::Fill::NextOpen,
                FillArg::Close => backtest::engine::Fill::Close,
            },
            capital: f.capital,
            fmt: f.out.into(),
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum CostsArg {
    /// India delivery for INR stocks, US for the rest
    Auto,
    /// No costs or slippage
    None,
    /// NSE delivery: brokerage, STT 0.1% both sides, stamp duty, DP charge
    #[value(alias = "delivery")]
    InDelivery,
    /// NSE intraday: brokerage, STT 0.025% on sells, stamp duty
    #[value(alias = "intraday")]
    InIntraday,
    /// US: commission-free, slippage only
    Us,
    /// Crypto exchanges: 0.1% fee per order, 10 bps slippage
    Crypto,
    /// Spot FX: 2 bps slippage for the spread
    Fx,
}

#[derive(Clone, Copy, ValueEnum)]
enum FillArg {
    /// Next bar's open (no look-ahead)
    NextOpen,
    /// Signal bar's close (how StockWise 0.3 and earlier filled)
    Close,
}

#[derive(Clone, Copy, ValueEnum)]
enum RebalanceArg {
    None,
    Monthly,
    Quarterly,
}

impl From<RebalanceArg> for backtest::engine::Rebalance {
    fn from(r: RebalanceArg) -> Self {
        match r {
            RebalanceArg::None => Self::None,
            RebalanceArg::Monthly => Self::Monthly,
            RebalanceArg::Quarterly => Self::Quarterly,
        }
    }
}

fn capital(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if v > 0.0 && v <= 1e12 => Ok(v),
        _ => Err(format!("`{s}` must be a number above 0 and at most 1e12")),
    }
}

fn slippage_bps(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(v) if (0.0..=1000.0).contains(&v) => Ok(v),
        _ => Err(format!(
            "`{s}` must be from 0 to 1000 basis points (1000 = 10%)"
        )),
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum DaemonMode {
    /// Paper-trading loop during market hours (IST; ET with `-m us`)
    Intraday,
    /// Daily post-market long-term scan
    Longterm,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportWhat {
    Portfolio,
    Watchlist,
}

#[derive(Clone, Copy, ValueEnum)]
enum ImportSource {
    /// Zerodha Kite holdings CSV
    #[value(alias = "zerodha")]
    Kite,
    /// INDmoney holdings CSV
    Indmoney,
    /// Groww stocks holdings export, saved as CSV
    Groww,
    /// Upstox holdings export (CSV)
    Upstox,
    /// CAMS / KFintech consolidated account statement (casparser JSON or CSV)
    #[value(alias = "cams", alias = "kfintech")]
    Cas,
    /// Any CSV with symbol, quantity and average-price columns
    Csv,
}

#[derive(Clone, Copy, ValueEnum)]
enum TaxCountry {
    /// India (STCG/LTCG)
    #[value(alias = "india")]
    In,
    /// United States
    #[value(alias = "usa")]
    Us,
    /// United Kingdom
    Uk,
}

#[derive(Clone, Copy, ValueEnum)]
enum AlertWhen {
    #[value(alias = ">")]
    Above,
    #[value(alias = "<")]
    Below,
    RsiBelow,
    RsiAbove,
    CrossAboveSma,
    CrossBelowSma,
    CrossAboveEma,
    CrossBelowEma,
    BreakoutHigh,
    BreakdownLow,
    ChangeAbove,
    ChangeBelow,
    VolumeSpike,
    ResultsIn,
}

/// A 3-letter currency code such as `usd` or `INR`, upper-cased.
fn parse_currency_code(value: &str) -> Result<String, String> {
    let code = value.trim().to_ascii_uppercase();
    if code.len() == 3 && code.chars().all(|c| c.is_ascii_alphabetic()) {
        Ok(code)
    } else {
        Err(format!(
            "`{value}` is not a 3-letter currency code (e.g. usd, inr)"
        ))
    }
}

/// The CLI spelling of a ValueEnum variant (e.g. `CostsArg::InDelivery` → "in-delivery"),
/// which is what the command handlers match on.
fn value_name<T: ValueEnum>(v: T) -> String {
    v.to_possible_value()
        .expect("no skipped variants")
        .get_name()
        .to_string()
}

/// Top-level help groups. Every subcommand must appear exactly once (tested).
const GROUPS: &[(&str, &[&str])] = &[
    (
        "Start here",
        &["quote", "deep", "markets", "dashboard", "search", "doctor"],
    ),
    (
        "Analysis",
        &[
            "analyze",
            "technical",
            "history",
            "chart",
            "compare",
            "risk",
            "correlate",
            "timeframes",
            "returns",
            "patterns",
            "support",
            "fibs",
            "volume",
            "gaps",
            "stoploss",
            "forecast",
        ],
    ),
    (
        "Market",
        &[
            "movers",
            "sectors",
            "heatmap",
            "sentiment",
            "news",
            "rotation",
            "sectorcmp",
            "peers",
            "insider",
            "ipo",
            "earnings",
            "surprise",
            "matrix",
            "picks",
            "screen",
            "dividends",
        ],
    ),
    ("Funds & F&O", &["mf", "options"]),
    ("Commodities & bonds", &["commodity", "bonds"]),
    (
        "Strategies & bots",
        &[
            "backtest", "sip", "whatif", "intraday", "longterm", "daemon", "sim", "report",
        ],
    ),
    (
        "Portfolio",
        &[
            "portfolio",
            "watch",
            "alert",
            "calendar",
            "export",
            "import",
            "wealth",
            "rebalance",
            "harvest",
            "tax",
            "divcal",
            "pcorr",
        ],
    ),
    ("Planning", &["goal"]),
    ("AI assistants", &["ask", "mcp"]),
    ("Interfaces", &["tui", "serve"]),
    ("Live trading", &["trade"]),
    ("Setup", &["notify", "config", "completions", "man", "help"]),
];

/// The catalog entry for a help group title. Titles without one (e.g. a
/// newly added group) show in English.
fn group_msg(english: &str) -> Option<i18n::Msg> {
    use i18n::Msg;
    Some(match english {
        "Start here" => Msg::GroupStartHere,
        "Analysis" => Msg::GroupAnalysis,
        "Market" => Msg::GroupMarket,
        "Strategies & bots" => Msg::GroupStrategies,
        "Funds & F&O" => Msg::GroupFunds,
        "Commodities & bonds" => Msg::GroupCommoditiesBonds,
        "Portfolio" => Msg::GroupPortfolio,
        "Interfaces" => Msg::GroupInterfaces,
        "Live trading" => Msg::GroupLiveTrading,
        "Setup" => Msg::GroupSetup,
        "Planning" => Msg::GroupPlanning,
        "AI assistants" => Msg::GroupAi,
        _ => return None,
    })
}

/// A help group title in the active language.
fn group_title(english: &str) -> &str {
    match group_msg(english) {
        Some(msg) => i18n::t(msg),
        None => english,
    }
}

/// Translated one-line descriptions for the most-used commands; the rest
/// show their English doc comment.
fn localized_about(name: &str) -> Option<&'static str> {
    use i18n::Msg;
    if i18n::lang() == i18n::Lang::En {
        return None;
    }
    let msg = match name {
        "quote" => Msg::AboutQuote,
        "deep" => Msg::AboutDeep,
        "markets" => Msg::AboutMarkets,
        "dashboard" => Msg::AboutDashboard,
        "search" => Msg::AboutSearch,
        "doctor" => Msg::AboutDoctor,
        "portfolio" => Msg::AboutPortfolio,
        "watch" => Msg::AboutWatch,
        "alert" => Msg::AboutAlert,
        "completions" => Msg::AboutCompletions,
        "help" => Msg::HelpPrintHelp,
        _ => return None,
    };
    Some(i18n::t(msg))
}

/// Top-level help with commands grouped by purpose (clap would otherwise list
/// 60 commands flat). Descriptions come from the doc comments above.
fn grouped_help_template(cmd: &clap::Command) -> String {
    use colored::Colorize;
    let mut out = format!(
        "{{before-help}}{{about-with-newline}}\n{} {{usage}}\n",
        i18n::t(i18n::Msg::HelpUsage).bold().underline()
    );
    for (group, names) in GROUPS {
        out.push_str(&format!(
            "\n{}\n",
            format!("{}:", group_title(group)).bold().underline()
        ));
        for name in *names {
            let about = localized_about(name)
                .map(str::to_string)
                .or_else(|| {
                    cmd.find_subcommand(name)
                        .and_then(|c| c.get_about())
                        .map(|a| a.to_string())
                })
                .unwrap_or_else(|| i18n::t(i18n::Msg::HelpPrintHelp).into());
            // Pad before styling so ANSI codes don't break the alignment.
            out.push_str(&format!("  {} {}\n", format!("{:<12}", name).bold(), about));
        }
    }
    out.push_str(&format!(
        "\n{}\n{{options}}{{after-help}}",
        i18n::t(i18n::Msg::HelpOptions).bold().underline()
    ));
    out
}

fn build_cli() -> clap::Command {
    let cmd = Cli::command();
    let template = grouped_help_template(&cmd);
    cmd.help_template(template)
}

/// Commands that take `-p/--period`, with their current default period.
fn period_defaults(cmd: &clap::Command) -> Vec<(String, String)> {
    cmd.get_subcommands()
        .filter_map(|sc| {
            let arg = sc.get_arguments().find(|a| a.get_id() == "period")?;
            let default = arg
                .get_default_values()
                .first()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default();
            Some((sc.get_name().to_string(), default))
        })
        .collect()
}

/// Make the config file's values the clap defaults, so flags and env vars
/// still win and `--help` shows the effective default. `cfg` is validated.
fn with_config_defaults(mut cmd: clap::Command, cfg: &config::Config) -> clap::Command {
    if let Some(m) = cfg
        .market
        .as_deref()
        .and_then(|m| config::market_value(m).ok())
    {
        cmd = cmd.mut_arg("market", |a| a.default_value(m));
    }
    if cfg.ai == Some(true) {
        cmd = cmd.mut_arg("ai", |a| a.default_value("true"));
    }
    // clap defaults are 'static; these few strings live for the whole run.
    let leak = |s: &str| -> &'static str { Box::leak(s.trim().to_string().into_boxed_str()) };
    if let Some(q) = cfg.quote_currency.as_deref() {
        let q = leak(q);
        cmd = cmd.mut_arg("quote_currency", |a| a.default_value(q));
    }
    if let Some(p) = cfg.portfolio.as_deref() {
        let p = leak(p);
        cmd = cmd.mut_arg("portfolio", |a| a.default_value(p));
    }
    for (name, period) in &cfg.periods {
        if let Ok(p) = config::period_value(period) {
            cmd = cmd.mut_subcommand(name, |sc| sc.mut_arg("period", |a| a.default_value(p)));
        }
    }
    cmd
}

/// Where a global option's value came from.
fn arg_source(matches: &clap::ArgMatches, id: &str, in_file: bool) -> config::Source {
    match matches.value_source(id) {
        Some(ValueSource::CommandLine) => config::Source::Flag,
        Some(ValueSource::EnvVariable) => config::Source::Env,
        _ if in_file => config::Source::File,
        _ => config::Source::Default,
    }
}

/// `stockwise man`: roff rendered from the clap definition, so the pages always
/// match the binary. Release archives ship the `--dir` output under `man/`.
fn cmd_man(dir: Option<&std::path::Path>) -> anyhow::Result<()> {
    use anyhow::Context;
    let Some(dir) = dir else {
        let mut page = Vec::new();
        clap_mangen::Man::new(Cli::command()).render(&mut page)?;
        print!("{}", String::from_utf8_lossy(&page));
        return Ok(());
    };
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    clap_mangen::generate_to(Cli::command(), dir)
        .with_context(|| format!("could not write man pages to {}", dir.display()))?;
    println!(
        "Wrote stockwise.1 and one page per command to {}",
        dir.display()
    );
    Ok(())
}

/// Exit quietly when stdout is closed early (`stockwise ... | head`) instead
/// of panicking with "failed printing to stdout: Broken pipe".
fn exit_quietly_on_broken_pipe() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");
        if msg.contains("Broken pipe") {
            std::process::exit(0);
        }
        default_hook(info);
    }));
}

#[tokio::main]
async fn main() {
    exit_quietly_on_broken_pipe();
    // The language is needed before parsing: the help text is built from it.
    // args_os: a non-UTF-8 argument is clap's to report, not a panic here.
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    // The config file comes first: it can set the language too.
    let period_cmds: Vec<String> = period_defaults(&Cli::command())
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    let period_refs: Vec<&str> = period_cmds.iter().map(String::as_str).collect();
    let cfg_state = config::install(config::load(&period_refs));
    let (lang, source) = i18n::detect_with_config(
        &args,
        |k| std::env::var(k).ok(),
        config::get().lang.as_deref(),
    );
    i18n::set_lang(lang, source);
    let cmd = with_config_defaults(build_cli(), config::get());
    let periods = period_defaults(&cmd);
    let matches = cmd.get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    // A broken config file stops everything except the commands that help fix it.
    if let config::Status::Invalid(msg) = &cfg_state.status
        && !matches!(
            cli.command,
            Commands::Config { .. } | Commands::Doctor | Commands::Completions { .. }
        )
    {
        eprintln!("\n  {} {}\n", colored::Colorize::red("Error:"), msg);
        std::process::exit(1);
    }

    let cfg = config::get();
    let sources = config::CliSources {
        market: arg_source(&matches, "market", cfg.market.is_some()),
        ai: if matches.value_source("no_ai") == Some(ValueSource::CommandLine) {
            config::Source::Flag
        } else {
            arg_source(&matches, "ai", cfg.ai.is_some())
        },
        quote_currency: arg_source(&matches, "quote_currency", cfg.quote_currency.is_some()),
        portfolio: arg_source(&matches, "portfolio", cfg.portfolio.is_some()),
    };
    let m = cli.market;
    ai::set_auto_ai(cli.ai && !cli.no_ai);
    market::set_quote_currency(&cli.quote_currency);
    if let Err(e) = portfolio::select(cli.portfolio.as_deref()) {
        eprintln!("\n  {} {:#}\n", colored::Colorize::red("Error:"), e);
        std::process::exit(2);
    }

    let result = match cli.command {
        Commands::Quote { symbols, out } => commands::cmd_quote(&symbols, m, out.into()).await,
        Commands::Deep { symbol, period } => commands::cmd_deep(&symbol, &period, m).await,
        Commands::Markets {
            out,
            crypto,
            fx,
            global,
        } => commands::cmd_markets(m, out.into(), crypto, fx, global).await,
        Commands::Dashboard => commands::cmd_dashboard(m).await,
        Commands::Search { query, out } => commands::cmd_search(&query, out.into()).await,
        Commands::Doctor => commands::cmd_doctor(m, sources).await,

        Commands::Analyze { symbol } => commands::cmd_analyze(&symbol, m).await,
        Commands::Technical {
            symbol,
            period,
            out,
        } => commands::cmd_technical(&symbol, &period, m, out.into()).await,
        Commands::History {
            symbol,
            period,
            json,
            csv,
        } => commands::cmd_history(&symbol, &period, m, Format::from_flags(json, csv)).await,
        Commands::Chart { symbol } => commands::cmd_chart(&symbol, m).await,
        Commands::Compare { symbols, out } => commands::cmd_compare(&symbols, m, out.into()).await,
        Commands::Risk {
            symbol,
            period,
            out,
        } => commands::cmd_risk(&symbol, &period, m, out.into()).await,
        Commands::Correlate { symbols, out } => {
            commands::cmd_correlate(&symbols, m, out.into()).await
        }
        Commands::Timeframes { symbol } => commands::cmd_multitimeframe(&symbol, m).await,
        Commands::Returns { symbol, out } => commands::cmd_returns(&symbol, m, out.into()).await,
        Commands::Patterns { symbol, period } => commands::cmd_patterns(&symbol, &period, m).await,
        Commands::Support { symbol, period } => commands::cmd_support(&symbol, &period, m).await,
        Commands::Fibs { symbol, period } => commands::cmd_fibs(&symbol, &period, m).await,
        Commands::Volume { symbol, period } => commands::cmd_volume(&symbol, &period, m).await,
        Commands::Gaps { symbol, period } => commands::cmd_gaps(&symbol, &period, m).await,
        Commands::Stoploss {
            symbol,
            entry,
            period,
        } => commands::cmd_stoploss(&symbol, entry, &period, m).await,
        Commands::Forecast {
            symbol,
            days,
            period,
        } => commands::cmd_forecast(&symbol, days, &period, m).await,

        Commands::Movers { universe, out } => commands::cmd_movers(m, universe, out.into()).await,
        Commands::Sectors { out } => commands::cmd_sectors(m, out.into()).await,
        Commands::Heatmap { universe } => commands::cmd_heatmap(m, universe).await,
        Commands::Sentiment => commands::cmd_sentiment().await,
        Commands::News { symbol } => commands::cmd_news(&symbol, m).await,
        Commands::Rotation => commands::cmd_rotation(m).await,
        Commands::Sectorcmp { sector1, sector2 } => {
            commands::cmd_sectorcmp(&sector1, &sector2, m).await
        }
        Commands::Peers { symbol } => commands::cmd_peers(&symbol, m).await,
        Commands::Insider { symbol } => commands::cmd_insider(&symbol, m).await,
        Commands::Ipo => commands::cmd_ipo().await,
        Commands::Earnings => commands::cmd_earnings(m).await,
        Commands::Surprise => commands::cmd_surprise(m).await,
        Commands::Matrix => commands::cmd_matrix(m).await,
        Commands::Picks => commands::cmd_picks(m).await,
        Commands::Screen {
            category,
            universe,
            out,
        } => commands::cmd_screen(&value_name(category), m, universe, out.into()).await,
        Commands::Dividends { symbol, out } => {
            commands::cmd_dividends(&symbol, m, out.into()).await
        }

        Commands::Commodity { action, out } => {
            use commands::CommodityView as V;
            let (view, out) = match action {
                None => (V::Spot, out),
                Some(CommodityCmd::Spot { out }) => (V::Spot, out),
                Some(CommodityCmd::Futures { out }) => (V::Futures, out),
                Some(CommodityCmd::Etfs { out }) => (V::Etfs, out),
                Some(CommodityCmd::Sgb { out }) => (V::Sgb, out),
            };
            commands::cmd_commodity(view, out.into()).await
        }
        Commands::Bonds { action, out } => match action {
            None => commands::cmd_bonds_yields(out.into()).await,
            Some(BondsCmd::Yields { out }) => commands::cmd_bonds_yields(out.into()).await,
            Some(BondsCmd::List { query, out }) => {
                commands::cmd_bonds_list(query.as_deref(), out.into()).await
            }
            Some(BondsCmd::Calc {
                security,
                coupon,
                maturity,
                price,
                ytm,
                settle,
                tbill,
                dirty,
                out,
            }) => {
                let input = commands::CalcInput {
                    security: security.as_deref(),
                    coupon,
                    maturity,
                    price,
                    ytm,
                    settle,
                    tbill,
                    dirty,
                };
                commands::cmd_bonds_calc(input, out.into()).await
            }
        },
        Commands::Mf { action } => match action {
            MfCmd::Search { query, limit, json } => {
                commands::cmd_mf_search(&query.join(" "), limit, json).await
            }
            MfCmd::Nav { codes, json } => commands::cmd_mf_nav(&codes, json).await,
            MfCmd::History { code, period, json } => {
                commands::cmd_mf_history(code, &period, json).await
            }
            MfCmd::Sip {
                code,
                amount,
                period,
                json,
            } => commands::cmd_mf_sip(code, amount, &period, json).await,
            MfCmd::Screen {
                category,
                plan,
                sort,
                max_ter,
                limit,
                max,
                json,
            } => {
                let plan = match plan {
                    MfPlan::Direct => commands::PlanFilter::Direct,
                    MfPlan::Regular => commands::PlanFilter::Regular,
                    MfPlan::All => commands::PlanFilter::All,
                };
                let sort = match sort {
                    MfSort::Y1 => commands::ScreenSort::Return(12),
                    MfSort::Y3 => commands::ScreenSort::Return(36),
                    MfSort::Y5 => commands::ScreenSort::Return(60),
                    MfSort::Ter => commands::ScreenSort::Ter,
                };
                commands::cmd_mf_screen(&category.join(" "), plan, sort, max_ter, limit, max, json)
                    .await
            }
            MfCmd::Overlap { a, b, json } => commands::cmd_mf_overlap(&a, &b, m, json).await,
        },
        Commands::Options {
            action,
            symbol,
            period,
        } => match action {
            Some(OptionsCmd::Chain {
                symbol,
                expiry,
                strikes,
                rate,
                div_yield,
                json,
            }) => {
                commands::cmd_options_chain(&symbol, expiry, strikes, rate, div_yield, m, json)
                    .await
            }
            Some(OptionsCmd::Pcr {
                symbol,
                expiry,
                json,
            }) => commands::cmd_options_pcr(&symbol, expiry, m, json).await,
            Some(OptionsCmd::Maxpain {
                symbol,
                expiry,
                json,
            }) => commands::cmd_options_maxpain(&symbol, expiry, m, json).await,
            Some(OptionsCmd::Payoff {
                symbol,
                legs,
                strategy,
                width,
                expiry,
                lots,
                lot_size,
                rate,
                div_yield,
                iv,
                today,
                json,
            }) => {
                let args = commands::PayoffArgs {
                    legs,
                    strategy: strategy.map(Into::into),
                    width,
                    expiry,
                    lots,
                    lot_size,
                    rate_pct: rate,
                    div_yield_pct: div_yield,
                    iv_pct: iv,
                    today,
                    json,
                };
                commands::cmd_options_payoff(&symbol, args, m).await
            }
            Some(OptionsCmd::Moves { symbol, period }) => {
                commands::cmd_options(&symbol, &period, m).await
            }
            None => {
                let symbol = symbol.expect("clap requires SYMBOL without a subcommand");
                commands::cmd_options(&symbol, &period, m).await
            }
        },

        Commands::Backtest(args) => match args.action {
            Some(BacktestCmd::Portfolio {
                symbols,
                weights,
                rebalance,
                period,
                flags,
            }) => {
                commands::cmd_backtest_portfolio(
                    &symbols,
                    weights,
                    rebalance.into(),
                    &period,
                    flags.into(),
                    m,
                )
                .await
            }
            Some(BacktestCmd::Walkforward {
                symbol,
                train,
                test,
                period,
                flags,
            }) => {
                commands::cmd_backtest_walkforward(&symbol, train, test, &period, flags.into(), m)
                    .await
            }
            Some(BacktestCmd::Strategies { files, json }) => {
                commands::cmd_backtest_strategies(&files, json)
            }
            None => {
                let symbol = args
                    .symbol
                    .expect("clap requires a symbol without a subcommand");
                commands::cmd_backtest(&symbol, &args.period, args.flags.into(), m).await
            }
        },
        Commands::Sip {
            symbol,
            amount,
            period,
            out,
        } => commands::cmd_sip(&symbol, amount, &period, m, out.into()).await,
        Commands::Whatif {
            symbol,
            amount,
            date,
            out,
        } => commands::cmd_whatif(&symbol, amount, &date, m, out.into()).await,
        Commands::Intraday {
            amount,
            target,
            strategy,
        } => commands::cmd_intraday(amount, target, strategy.as_deref(), m).await,
        Commands::Longterm { amount } => commands::cmd_longterm(amount, m).await,
        Commands::Daemon { mode, amount } => {
            commands::cmd_daemon(&value_name(mode), amount, m).await
        }
        Commands::Sim { action } => match action {
            Some(SimCmd::Reset { yes }) => commands::cmd_sim_reset(yes),
            Some(SimCmd::Start { amount, target }) => {
                commands::cmd_sim("start", amount, target, m).await
            }
            Some(SimCmd::Status) => commands::cmd_sim("status", None, None, m).await,
            Some(SimCmd::Settle { now }) => commands::cmd_sim_settle(now).await,
            Some(SimCmd::History { out }) => commands::cmd_sim_history(m, out.into()),
            Some(SimCmd::Replay { action }) => match action {
                ReplayCmd::List { out } => commands::cmd_replay_list(out.into()),
                ReplayCmd::Start {
                    scenario,
                    from,
                    to,
                    symbols,
                    benchmark,
                    capital,
                    costs,
                    out,
                } => {
                    let start = commands::ReplayStart {
                        scenario,
                        from,
                        to,
                        symbols,
                        benchmark,
                        capital,
                        costs,
                        fmt: out.into(),
                    };
                    commands::cmd_replay_start(start, m).await
                }
                ReplayCmd::Next { days, out } => commands::cmd_replay_next(days, out.into()),
                ReplayCmd::Buy {
                    symbol,
                    qty,
                    stop,
                    target,
                    out,
                } => commands::cmd_replay_order(
                    commands::OrderSide::Buy,
                    &symbol,
                    Some(qty),
                    stop,
                    target,
                    out.into(),
                ),
                ReplayCmd::Sell { symbol, qty, out } => commands::cmd_replay_order(
                    commands::OrderSide::Sell,
                    &symbol,
                    qty,
                    None,
                    None,
                    out.into(),
                ),
                ReplayCmd::Cancel { id } => commands::cmd_replay_cancel(id),
                ReplayCmd::Set {
                    symbol,
                    stop,
                    target,
                } => commands::cmd_replay_set(&symbol, stop, target),
                ReplayCmd::Status { out } => commands::cmd_replay_status(out.into()),
                ReplayCmd::Chart { symbol, days } => commands::cmd_replay_chart(&symbol, days),
                ReplayCmd::End { out } => commands::cmd_replay_end(out.into()),
            },
            Some(SimCmd::Challenge { action }) => match action {
                ChallengeCmd::List { out } => commands::cmd_challenge_list(out.into()),
                ChallengeCmd::Start { name, out } => {
                    commands::cmd_challenge_start(&name, m, out.into())
                }
                ChallengeCmd::Status { out } => commands::cmd_challenge_status(out.into()).await,
                ChallengeCmd::End { out } => commands::cmd_challenge_end(out.into()).await,
            },
            Some(SimCmd::Profile { out }) => commands::cmd_sim_profile(out.into()),
            Some(SimCmd::Card { svg, text }) => commands::cmd_sim_card(svg, text),
            None => commands::cmd_sim("help", None, None, m).await,
        },
        Commands::Report { what } => commands::cmd_report(&what, m).await,

        Commands::Portfolio { base, action, out } => match action {
            Some(PortfolioCmd::Add {
                symbol,
                shares,
                cost,
                date,
            }) => commands::cmd_portfolio_add(&symbol, shares, cost, date.as_deref(), m).await,
            Some(PortfolioCmd::Sell {
                symbol,
                shares,
                price,
                date,
            }) => commands::cmd_portfolio_sell(&symbol, shares, price, date.as_deref(), m).await,
            Some(PortfolioCmd::Lots { symbol, lot, date }) => {
                commands::cmd_portfolio_lots(&symbol, lot, date.as_deref(), m).await
            }
            Some(PortfolioCmd::Actions { symbol, apply }) => {
                commands::cmd_portfolio_actions(symbol.as_deref(), apply, m).await
            }
            Some(PortfolioCmd::Remove { symbol }) => {
                commands::cmd_portfolio_remove(&symbol, m).await
            }
            Some(PortfolioCmd::List) => commands::cmd_portfolio_list(),
            Some(PortfolioCmd::Benchmark {
                benchmark,
                period,
                out: bench,
            }) => {
                let base = base.map(|b| value_name(b).to_uppercase());
                commands::cmd_portfolio_benchmark(
                    benchmark.as_deref(),
                    &period,
                    m,
                    base.as_deref(),
                    bench.into(),
                )
                .await
            }
            Some(PortfolioCmd::Show { out: show }) => {
                let base = base.map(|b| value_name(b).to_uppercase());
                commands::cmd_portfolio_show(m, base.as_deref(), out.or(show)).await
            }
            None => {
                let base = base.map(|b| value_name(b).to_uppercase());
                commands::cmd_portfolio_show(m, base.as_deref(), out.into()).await
            }
        },
        Commands::Watch { action, out } => match action {
            Some(WatchCmd::Add { symbols }) => commands::cmd_watchlist("add", &symbols, m).await,
            Some(WatchCmd::Remove { symbols }) => {
                commands::cmd_watchlist("remove", &symbols, m).await
            }
            Some(WatchCmd::Show { out: show }) => commands::cmd_watchlist_show(out.or(show)).await,
            None => commands::cmd_watchlist_show(out.into()).await,
        },
        Commands::Alert { action, out } => match action {
            Some(AlertCmd::Add {
                symbol,
                condition,
                target,
                period,
            }) => {
                let condition = value_name(condition);
                let period = period.map(|p| p as usize);
                commands::cmd_alert_add(&symbol, &condition, target, period, m).await
            }
            Some(AlertCmd::Remove { index }) => commands::cmd_alert_remove(index),
            Some(AlertCmd::Check { out: check }) => commands::cmd_alert_check(out.or(check)).await,
            Some(AlertCmd::List { out: list }) => commands::cmd_alert_list(out.or(list)),
            Some(AlertCmd::Watch { interval, once }) => {
                commands::cmd_alert_watch(interval, once, m).await
            }
            None => commands::cmd_alert_list(out.into()),
        },
        Commands::Calendar {
            symbols,
            days,
            holdings,
            watchlist,
            out,
        } => {
            let scope = match (holdings, watchlist) {
                (true, _) => commands::CalendarScope::Portfolio,
                (_, true) => commands::CalendarScope::Watchlist,
                _ => commands::CalendarScope::Both,
            };
            commands::cmd_calendar(&symbols, days, scope, m, out.into()).await
        }
        Commands::Export { what } => commands::cmd_export(&value_name(what), m).await,
        Commands::Import {
            source,
            file,
            replace,
        } => commands::cmd_import(&value_name(source), &file, replace, m).await,
        Commands::Wealth => commands::cmd_wealth(m).await,
        Commands::Rebalance => commands::cmd_rebalance(&[], m).await,
        Commands::Harvest => commands::cmd_harvest(m).await,
        Commands::Tax {
            action,
            country,
            out,
        } => match action {
            Some(TaxCmd::Export {
                country,
                fy,
                out,
                json,
            }) => {
                let country = match country {
                    Some(c) => value_name(c),
                    None if m == Market::Us => "us".into(),
                    None => "in".into(),
                };
                commands::cmd_tax_export(&country, fy.as_deref(), out.as_deref(), json).await
            }
            None => {
                let country = country.expect("clap requires COUNTRY without a subcommand");
                commands::cmd_tax(&value_name(country), m, out.into()).await
            }
        },
        Commands::Goal { action } => match action {
            GoalCmd::Sip {
                target,
                years,
                return_pct,
                sip,
            } => commands::cmd_goal_sip(
                target,
                years,
                return_pct,
                sip.step_up,
                sip.savings.unwrap_or(0.0),
                m,
                sip.out.json,
            ),
            GoalCmd::Retire {
                monthly_expense,
                years_to_retire,
                years_in_retirement,
                inflation,
                return_pre,
                return_post,
                sip,
            } => commands::cmd_goal_retire(
                monthly_expense,
                years_to_retire,
                years_in_retirement,
                inflation,
                return_pre,
                return_post,
                sip.step_up,
                sip.savings.unwrap_or(0.0),
                m,
                sip.out.json,
            ),
            GoalCmd::Education {
                cost,
                years,
                inflation,
                return_pct,
                sip,
            } => commands::cmd_goal_education(
                cost,
                years,
                inflation,
                return_pct,
                sip.step_up,
                sip.savings.unwrap_or(0.0),
                m,
                sip.out.json,
            ),
        },
        Commands::Divcal => commands::cmd_divcal(m).await,
        Commands::Pcorr => commands::cmd_pcorr(m).await,

        Commands::Trade { broker, action } => {
            use brokers::Side;
            use commands::TradeAction as A;
            let order = |side, symbol, qty, prices: PriceFlags, flags: OrderFlags| A::Order {
                side,
                symbol,
                qty,
                limit: prices.limit,
                trigger: prices.trigger,
                opts: flags.into(),
            };
            let gtt = |side, symbol, qty, trigger, limit, flags: OrderFlags| A::GttCreate {
                side,
                symbol,
                qty,
                trigger,
                limit,
                opts: flags.into(),
            };
            let action = match action {
                TradeCmd::Setup => A::Setup,
                TradeCmd::Config { keyring } => A::Config { keyring },
                TradeCmd::Login => A::Login,
                TradeCmd::Holdings => A::Holdings,
                TradeCmd::Positions => A::Positions,
                TradeCmd::Orders => A::Orders,
                TradeCmd::Buy {
                    symbol,
                    qty,
                    prices,
                    flags,
                } => order(Side::Buy, symbol, qty, prices, flags),
                TradeCmd::Sell {
                    symbol,
                    qty,
                    prices,
                    flags,
                } => order(Side::Sell, symbol, qty, prices, flags),
                TradeCmd::Limit {
                    symbol,
                    qty,
                    price,
                    flags,
                } => order(
                    Side::Buy,
                    symbol,
                    qty,
                    PriceFlags {
                        limit: Some(price),
                        trigger: None,
                    },
                    flags,
                ),
                TradeCmd::Gtt { action } => match action {
                    GttCmd::Buy {
                        symbol,
                        qty,
                        trigger,
                        limit,
                        flags,
                    } => gtt(Side::Buy, symbol, qty, trigger, limit, flags),
                    GttCmd::Sell {
                        symbol,
                        qty,
                        trigger,
                        limit,
                        flags,
                    } => gtt(Side::Sell, symbol, qty, trigger, limit, flags),
                    GttCmd::List { notify, out } => A::GttList {
                        notify,
                        format: out.into(),
                    },
                    GttCmd::Cancel { id, flags } => A::GttCancel {
                        id,
                        opts: flags.into(),
                    },
                },
                TradeCmd::Selftest {
                    redact_amounts,
                    out,
                } => A::Selftest {
                    redact_amounts,
                    format: out.into(),
                },
                TradeCmd::Sync {
                    since,
                    apply,
                    notify,
                    out,
                } => A::Sync {
                    since,
                    apply,
                    notify,
                    format: out.into(),
                },
            };
            commands::cmd_trade(broker, action, m).await
        }

        Commands::Tui { interval, snapshot } => {
            commands::cmd_tui(interval, snapshot.as_deref(), m).await
        }
        Commands::Serve {
            bind,
            allow_remote,
            open,
        } => commands::cmd_serve(&bind, allow_remote, open, m).await,
        Commands::Ask {
            question,
            max_steps,
            max_tokens,
            verbose,
            out,
        } => {
            let opts = ask::AskOptions {
                max_steps: max_steps as usize,
                token_budget: max_tokens,
                verbose,
                ..Default::default()
            };
            commands::cmd_ask(&question.join(" "), opts, m, out.json).await
        }
        Commands::Mcp {
            http,
            allow_remote,
            allow_orders,
            list_tools,
        } => commands::cmd_mcp(http.as_deref(), allow_remote, allow_orders, list_tools, m).await,
        Commands::Config { action } => match action {
            Some(ConfigCmd::Path) => commands::cmd_config_path(),
            Some(ConfigCmd::Init { force }) => commands::cmd_config_init(force),
            Some(ConfigCmd::Show { out }) => {
                commands::cmd_config_show(m, sources, &periods, out.into())
            }
            None => commands::cmd_config_show(m, sources, &periods, Format::Table),
        },

        Commands::Notify { action } => match action {
            Some(NotifyCmd::Test) => commands::cmd_notify("test").await,
            Some(NotifyCmd::Status) | None => commands::cmd_notify("status").await,
        },

        Commands::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "stockwise",
                &mut std::io::stdout(),
            );
            Ok(())
        }
        Commands::Man { dir } => cmd_man(dir.as_deref()),
    };

    if let Err(e) = result {
        eprintln!(
            "\n  {} {:#}\n",
            colored::Colorize::red(i18n::t(i18n::Msg::ErrorPrefix)),
            e
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_command_is_in_exactly_one_help_group() {
        let cmd = Cli::command();
        let mut names: Vec<String> = cmd
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        names.push("help".into());
        for name in &names {
            let hits = GROUPS
                .iter()
                .filter(|(_, cmds)| cmds.contains(&name.as_str()))
                .count();
            assert_eq!(hits, 1, "`{name}` must be in exactly one help group");
        }
        let grouped: usize = GROUPS.iter().map(|(_, cmds)| cmds.len()).sum();
        assert_eq!(grouped, names.len(), "help groups list an unknown command");
    }

    #[test]
    fn man_page_lists_every_command() {
        let mut page = Vec::new();
        clap_mangen::Man::new(Cli::command())
            .render(&mut page)
            .unwrap();
        let page = String::from_utf8(page).unwrap();
        for sub in Cli::command().get_subcommands() {
            let reference = format!("stockwise\\-{}(1)", sub.get_name());
            assert!(page.contains(&reference), "man page is missing {reference}");
        }
    }

    #[test]
    fn man_dir_writes_a_page_per_command() {
        let dir = std::env::temp_dir().join(format!("stockwise-man-{}", std::process::id()));
        cmd_man(Some(&dir)).unwrap();
        for page in [
            "stockwise.1",
            "stockwise-quote.1",
            "stockwise-portfolio-add.1",
            "stockwise-trade-buy.1",
        ] {
            assert!(dir.join(page).is_file(), "{page} was not written");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn commodity_and_bonds_commands_parse() {
        for args in [
            vec!["stockwise", "commodity"],
            vec!["stockwise", "commodity", "--json"],
            vec!["stockwise", "commodity", "spot", "--csv"],
            vec!["stockwise", "commodity", "futures"],
            vec!["stockwise", "commodity", "etfs", "--json"],
            vec!["stockwise", "commodity", "sgb", "--csv"],
            vec!["stockwise", "bonds"],
            vec!["stockwise", "bonds", "--csv"],
            vec!["stockwise", "bonds", "yields", "--json"],
            vec!["stockwise", "bonds", "list", "2033"],
            vec![
                "stockwise",
                "bonds",
                "calc",
                "718GS2033",
                "--price",
                "101.25",
            ],
            vec!["stockwise", "bonds", "calc", "718GS2033", "--ytm", "-0.5"],
            vec![
                "stockwise",
                "bonds",
                "calc",
                "--coupon",
                "7.1%",
                "--maturity",
                "2034-04-08",
                "--price",
                "99.5",
                "--dirty",
                "--json",
            ],
            vec![
                "stockwise",
                "bonds",
                "calc",
                "--tbill",
                "--maturity",
                "2027-01-14",
                "--price",
                "98.6",
            ],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        for args in [
            // Neither a price nor a yield.
            vec!["stockwise", "bonds", "calc", "718GS2033"],
            // Both.
            vec![
                "stockwise",
                "bonds",
                "calc",
                "718GS2033",
                "--price",
                "100",
                "--ytm",
                "7",
            ],
            // No security and no maturity.
            vec!["stockwise", "bonds", "calc", "--price", "100"],
            vec!["stockwise", "bonds", "calc", "X", "--price", "-1"],
            vec![
                "stockwise",
                "bonds",
                "calc",
                "X",
                "--price",
                "99",
                "--tbill",
                "--dirty",
            ],
            vec!["stockwise", "commodity", "spot", "sgb"],
        ] {
            assert!(Cli::try_parse_from(&args).is_err(), "{args:?} should fail");
        }
    }

    #[test]
    fn global_and_commodity_markets_parse() {
        use market::Exchange;
        for (flag, want) in [
            ("uk", Market::Global(Exchange::Uk)),
            ("UK", Market::Global(Exchange::Uk)),
            ("lse", Market::Global(Exchange::Uk)),
            ("de", Market::Global(Exchange::De)),
            ("jp", Market::Global(Exchange::Jp)),
            ("hk", Market::Global(Exchange::Hk)),
            ("sg", Market::Global(Exchange::Sg)),
            ("ca", Market::Global(Exchange::Ca)),
            ("au", Market::Global(Exchange::Au)),
            ("commodity", Market::Commodity),
        ] {
            let cli = Cli::try_parse_from(["stockwise", "quote", "X", "-m", flag]).unwrap();
            assert_eq!(cli.market, want, "-m {flag}");
        }
        assert!(Cli::try_parse_from(["stockwise", "quote", "X", "-m", "mars"]).is_err());
        let cli = Cli::try_parse_from(["stockwise", "markets", "--global"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Markets { global: true, .. }
        ));
    }

    #[test]
    fn crypto_fx_lang_and_quote_currency_flags_parse() {
        for args in [
            vec!["stockwise", "quote", "BTC", "-m", "crypto"],
            vec![
                "stockwise",
                "quote",
                "ETH",
                "-m",
                "crypto",
                "--quote-currency",
                "inr",
            ],
            vec!["stockwise", "quote", "USDINR", "-m", "fx"],
            vec!["stockwise", "quote", "USDINR", "-m", "forex"],
            vec!["stockwise", "markets", "--crypto", "--fx"],
            vec!["stockwise", "--lang", "hi", "dashboard"],
            vec!["stockwise", "portfolio", "--lang", "HI"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        assert!(
            Cli::try_parse_from(["stockwise", "quote", "BTC", "--quote-currency", "rupees"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["stockwise", "--lang", "fr", "quote", "TCS"]).is_err());
        let cli =
            Cli::try_parse_from(["stockwise", "quote", "BTC", "--quote-currency", "inr"]).unwrap();
        assert_eq!(cli.quote_currency, "INR");
    }

    #[test]
    fn help_template_translates_group_titles() {
        let template = grouped_help_template(&Cli::command());
        // English unless set: tests never call i18n::set_lang.
        assert!(template.contains("Start here"));
        for (group, _) in GROUPS {
            let msg = group_msg(group).unwrap_or_else(|| panic!("`{group}` has no translation"));
            assert_eq!(msg.text(i18n::Lang::En), *group);
        }
        assert_eq!(group_title("Some new group"), "Some new group");
    }

    #[test]
    fn quote_requires_a_symbol() {
        assert!(Cli::try_parse_from(["stockwise", "quote"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "quote", "TCS"]).is_ok());
    }

    #[test]
    fn compare_requires_two_symbols() {
        assert!(Cli::try_parse_from(["stockwise", "compare", "AAPL"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "compare", "AAPL", "MSFT"]).is_ok());
    }

    #[test]
    fn bad_period_is_rejected_at_parse_time() {
        assert!(Cli::try_parse_from(["stockwise", "history", "TCS", "-p", "7x"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "history", "TCS", "-p", "6mo"]).is_ok());
    }

    /// Split a shell command line the way a user would type it: honour
    /// double quotes and stop at a pipe or redirect.
    fn shell_words(line: &str) -> Vec<String> {
        let (mut words, mut cur, mut quoted, mut has) = (Vec::new(), String::new(), false, false);
        for ch in line.chars() {
            match ch {
                '"' => {
                    quoted = !quoted;
                    has = true;
                }
                '|' | '>' if !quoted => break,
                c if c.is_whitespace() && !quoted => {
                    if has {
                        words.push(std::mem::take(&mut cur));
                        has = false;
                    }
                }
                c => {
                    cur.push(c);
                    has = true;
                }
            }
        }
        if has {
            words.push(cur);
        }
        words
    }

    /// Parse every `stockwise ...` code span in a Markdown file; returns how
    /// many were checked (0 when the file isn't there).
    fn check_doc_commands(file: &str) -> usize {
        // Read at run time: docs/ and skills/ are not in the crates.io package.
        let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), file);
        let Ok(text) = std::fs::read_to_string(path) else {
            return 0;
        };
        let mut checked = 0;
        for span in text.split('`').skip(1).step_by(2) {
            // Skip prose placeholders such as `stockwise <command> --help`.
            if !span.starts_with("stockwise ") || span.contains('<') || span.contains('…') {
                continue;
            }
            let span = span.replace("\\|", "|");
            let args = shell_words(&span);
            if let Err(e) = Cli::try_parse_from(&args) {
                use clap::error::ErrorKind;
                if !matches!(e.kind(), ErrorKind::DisplayVersion | ErrorKind::DisplayHelp) {
                    panic!("{file}: `{span}` does not parse:\n{e}");
                }
            }
            checked += 1;
        }
        checked
    }

    #[test]
    fn cheatsheet_commands_parse() {
        let checked = check_doc_commands("docs/cheatsheet.md");
        assert!(
            checked == 0 || checked > 40,
            "only {checked} commands found in the cheatsheet"
        );
    }

    #[test]
    fn agent_skill_commands_parse() {
        let checked = check_doc_commands("skills/stockwise/SKILL.md");
        assert!(
            checked == 0 || checked > 15,
            "only {checked} commands found in the Agent Skill"
        );
    }

    #[test]
    fn legacy_positional_forms_still_parse() {
        for args in [
            vec!["stockwise", "portfolio", "add", "TCS", "10", "3500"],
            vec!["stockwise", "portfolio", "show"],
            vec!["stockwise", "watch", "add", "TCS", "INFY"],
            vec!["stockwise", "alert", "add", "TCS", "above", "4200"],
            vec!["stockwise", "alert", "add", "TCS", ">", "4200"],
            vec!["stockwise", "sim", "start", "100000", "2"],
            vec!["stockwise", "trade", "buy", "RELIANCE", "10"],
            vec!["stockwise", "trade", "limit", "RELIANCE", "10", "1300"],
            vec!["stockwise", "trade", "config"],
            vec!["stockwise", "trade", "holdings"],
            vec!["stockwise", "import", "zerodha", "holdings.csv"],
            vec!["stockwise", "tax", "india"],
            vec!["stockwise", "daemon", "intraday", "25000"],
            vec!["stockwise", "backtest", "TCS", "-s", "bb"],
            vec!["stockwise", "backtest", "TCS", "-s", "bollinger"],
            vec!["stockwise", "intraday", "50000", "1.5", "-s", "rsi"],
            vec!["stockwise", "sim", "start"],
            vec!["stockwise", "tax", "US"],
            vec!["stockwise", "tax", "India"],
            vec!["stockwise", "screen", "Growth"],
            vec!["stockwise", "history", "TCS", "-p", "ytd"],
            vec!["stockwise", "options", "TCS"],
            vec!["stockwise", "options", "AAPL", "-p", "3mo", "-m", "us"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
    }

    #[test]
    fn backtest_forms_parse() {
        let parse = |args: &[&str]| Cli::try_parse_from(args).map(|c| c.command);

        // The original single-symbol form still works.
        match parse(&["stockwise", "backtest", "TCS", "-s", "rsi"]) {
            Ok(Commands::Backtest(a)) => {
                assert!(a.action.is_none());
                assert_eq!(a.symbol.as_deref(), Some("TCS"));
                assert_eq!(a.flags.strategy, "rsi");
                assert_eq!(a.period, "2y");
            }
            _ => panic!("backtest TCS -s rsi should parse as a single-symbol backtest"),
        }
        assert!(parse(&["stockwise", "backtest"]).is_err());

        match parse(&[
            "stockwise",
            "backtest",
            "portfolio",
            "TCS",
            "INFY",
            "-s",
            "sma",
            "--weights",
            "0.6,0.4",
            "--rebalance",
            "monthly",
            "--json",
        ]) {
            Ok(Commands::Backtest(a)) => match a.action {
                Some(BacktestCmd::Portfolio {
                    symbols,
                    weights,
                    flags,
                    ..
                }) => {
                    assert_eq!(symbols, ["TCS", "INFY"]);
                    assert_eq!(weights, backtest::engine::Weights::Fixed(vec![0.6, 0.4]));
                    assert!(flags.out.json);
                }
                _ => panic!("expected backtest portfolio"),
            },
            _ => panic!("backtest portfolio should parse"),
        }
        assert!(parse(&["stockwise", "backtest", "portfolio"]).is_err());

        match parse(&[
            "stockwise",
            "backtest",
            "walkforward",
            "INFY",
            "--train",
            "1y",
            "--test",
            "3mo",
        ]) {
            Ok(Commands::Backtest(a)) => match a.action {
                Some(BacktestCmd::Walkforward { train, test, .. }) => {
                    assert_eq!((train.bars(252.0), test.bars(252.0)), (252, 63));
                }
                _ => panic!("expected backtest walkforward"),
            },
            _ => panic!("backtest walkforward should parse"),
        }

        for args in [
            vec!["stockwise", "backtest", "strategies"],
            vec!["stockwise", "backtest", "strategies", "my.toml", "--json"],
            vec!["stockwise", "backtest", "wf", "TCS"],
            vec![
                "stockwise",
                "backtest",
                "TCS",
                "-s",
                "file:my.toml",
                "-P",
                "oversold=25",
            ],
            vec![
                "stockwise",
                "backtest",
                "TCS",
                "--costs",
                "in-intraday",
                "--fill",
                "close",
            ],
            vec![
                "stockwise",
                "backtest",
                "TCS",
                "--costs",
                "none",
                "--capital",
                "50000",
            ],
            vec!["stockwise", "backtest", "TCS", "--brokerage", "20,0.03%"],
            vec!["stockwise", "backtest", "AAPL", "-m", "us", "--costs", "us"],
            vec!["stockwise", "backtest", "portfolio", "TCS", "INFY", "--csv"],
            vec!["stockwise", "backtest", "wf", "TCS", "--csv"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        for args in [
            vec!["stockwise", "backtest", "TCS", "--costs", "zerodha"],
            vec!["stockwise", "backtest", "TCS", "--fill", "open"],
            vec!["stockwise", "backtest", "TCS", "--capital", "0"],
            vec!["stockwise", "backtest", "TCS", "--capital", "1e300"],
            vec!["stockwise", "backtest", "TCS", "--slippage-bps", "20000"],
            vec!["stockwise", "backtest", "TCS", "--slippage-bps", "-1"],
            vec!["stockwise", "backtest", "TCS", "--brokerage", "lots"],
            vec!["stockwise", "backtest", "TCS", "-P", "oversold"],
            vec![
                "stockwise",
                "backtest",
                "walkforward",
                "TCS",
                "--train",
                "1q",
            ],
            vec![
                "stockwise",
                "backtest",
                "portfolio",
                "A",
                "B",
                "--weights",
                "x",
            ],
            vec!["stockwise", "backtest", "TCS", "portfolio"],
            vec!["stockwise", "backtest", "TCS", "--json", "--csv"],
        ] {
            assert!(Cli::try_parse_from(&args).is_err(), "{args:?} should fail");
        }
    }

    #[test]
    fn bots_and_alerts_forms_parse() {
        for args in [
            vec!["stockwise", "sim", "settle"],
            vec!["stockwise", "sim", "settle", "--now"],
            vec!["stockwise", "alert", "watch"],
            vec!["stockwise", "alert", "watch", "--interval", "30", "--once"],
            vec!["stockwise", "notify"],
            vec!["stockwise", "notify", "test"],
            vec!["stockwise", "-m", "us", "daemon", "intraday", "5000"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        // Faster than every 10 seconds would hammer Yahoo.
        assert!(Cli::try_parse_from(["stockwise", "alert", "watch", "--interval", "5"]).is_err());
    }

    #[test]
    fn practice_commands_parse() {
        for args in [
            vec!["stockwise", "sim", "replay", "list", "--json"],
            vec!["stockwise", "sim", "replay", "scenarios"],
            vec!["stockwise", "sim", "replay", "start", "covid-crash-2020"],
            vec![
                "stockwise",
                "sim",
                "replay",
                "start",
                "file:my.toml",
                "--capital",
                "50000",
            ],
            vec![
                "stockwise",
                "sim",
                "replay",
                "start",
                "--from",
                "2022-01-03",
                "--to",
                "2022-06-30",
                "--symbols",
                "TCS,INFY",
                "--costs",
                "none",
                "--json",
            ],
            vec!["stockwise", "sim", "replay", "next"],
            vec!["stockwise", "sim", "replay", "next", "5", "--json"],
            vec![
                "stockwise",
                "sim",
                "replay",
                "buy",
                "TCS",
                "10",
                "--stop",
                "3200",
                "--target",
                "3900",
            ],
            vec!["stockwise", "sim", "replay", "sell", "TCS"],
            vec!["stockwise", "sim", "replay", "sell", "TCS", "5", "--json"],
            vec!["stockwise", "sim", "replay", "cancel"],
            vec!["stockwise", "sim", "replay", "cancel", "3"],
            vec!["stockwise", "sim", "replay", "set", "TCS", "--stop", "3300"],
            vec!["stockwise", "sim", "replay", "status", "--json"],
            vec![
                "stockwise",
                "sim",
                "replay",
                "chart",
                "index",
                "--days",
                "30",
            ],
            vec!["stockwise", "sim", "replay", "end", "--json"],
            vec!["stockwise", "sim", "challenge", "list"],
            vec![
                "stockwise",
                "sim",
                "challenge",
                "start",
                "steady-hands",
                "--json",
            ],
            vec!["stockwise", "sim", "challenge", "status"],
            vec!["stockwise", "sim", "challenge", "end", "--json"],
            vec!["stockwise", "sim", "profile", "--json"],
            vec!["stockwise", "sim", "card"],
            vec!["stockwise", "sim", "card", "--svg", "card.svg", "--text"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        for args in [
            vec!["stockwise", "sim", "replay", "start"],
            vec![
                "stockwise",
                "sim",
                "replay",
                "start",
                "covid-crash-2020",
                "--from",
                "2020-01-01",
                "--to",
                "2020-02-01",
            ],
            vec![
                "stockwise",
                "sim",
                "replay",
                "start",
                "--from",
                "2022-01-03",
            ],
            vec![
                "stockwise",
                "sim",
                "replay",
                "start",
                "--from",
                "03/01/2022",
                "--to",
                "2022-06-30",
            ],
            vec![
                "stockwise",
                "sim",
                "replay",
                "start",
                "x",
                "--costs",
                "free",
            ],
            vec!["stockwise", "sim", "replay", "next", "0"],
            vec!["stockwise", "sim", "replay", "buy", "TCS"],
            vec!["stockwise", "sim", "replay", "buy", "TCS", "0"],
            vec![
                "stockwise",
                "sim",
                "replay",
                "buy",
                "TCS",
                "5",
                "--stop",
                "-1",
            ],
            vec![
                "stockwise",
                "sim",
                "replay",
                "sell",
                "TCS",
                "5",
                "--stop",
                "100",
            ],
            vec!["stockwise", "sim", "challenge", "start"],
        ] {
            assert!(Cli::try_parse_from(&args).is_err(), "{args:?} should fail");
        }
    }

    #[test]
    fn technical_alerts_and_calendar_parse() {
        for args in [
            vec!["stockwise", "alert", "add", "INFY", "rsi-below", "30"],
            vec![
                "stockwise",
                "alert",
                "add",
                "INFY",
                "RSI-ABOVE",
                "70",
                "--period",
                "9",
            ],
            vec!["stockwise", "alert", "add", "TCS", "cross-above-sma", "200"],
            vec!["stockwise", "alert", "add", "TCS", "cross-below-ema", "20"],
            vec![
                "stockwise",
                "-m",
                "us",
                "alert",
                "add",
                "AAPL",
                "breakout-high",
                "52",
            ],
            vec!["stockwise", "alert", "add", "TCS", "breakdown-low", "20"],
            vec!["stockwise", "alert", "add", "TCS", "change-above", "3"],
            vec!["stockwise", "alert", "add", "TCS", "change-below", "-3"],
            vec!["stockwise", "alert", "add", "TCS", "volume-spike", "2.5"],
            vec!["stockwise", "alert", "add", "TCS", "results-in", "7"],
            vec!["stockwise", "calendar"],
            vec!["stockwise", "calendar", "--days", "14", "--holdings"],
            vec![
                "stockwise",
                "calendar",
                "--holdings",
                "--portfolio",
                "long-term",
            ],
            vec!["stockwise", "calendar", "--watchlist", "--json"],
            vec!["stockwise", "calendar", "TCS", "INFY", "--csv"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        for args in [
            vec!["stockwise", "alert", "add", "TCS", "sideways", "3"],
            vec![
                "stockwise",
                "alert",
                "add",
                "TCS",
                "rsi-below",
                "30",
                "--period",
                "1",
            ],
            vec!["stockwise", "calendar", "--days", "0"],
            vec!["stockwise", "calendar", "--holdings", "--watchlist"],
            vec!["stockwise", "calendar", "TCS", "--holdings"],
        ] {
            assert!(Cli::try_parse_from(&args).is_err(), "{args:?} should fail");
        }
    }

    #[test]
    fn every_cli_alert_condition_is_a_stored_condition() {
        for v in AlertWhen::value_variants() {
            let name = value_name(*v);
            assert!(
                alerts::AlertCondition::parse(&name).is_some(),
                "{name} has no AlertCondition"
            );
        }
        assert_eq!(
            AlertWhen::value_variants().len(),
            alerts::AlertCondition::ALL.len()
        );
    }

    #[test]
    fn portfolio_lot_commands_parse() {
        for args in [
            vec![
                "stockwise",
                "portfolio",
                "add",
                "TCS",
                "10",
                "3500",
                "--date",
                "2023-04-12",
            ],
            vec!["stockwise", "portfolio", "sell", "TCS", "5"],
            vec![
                "stockwise",
                "portfolio",
                "sell",
                "TCS",
                "5",
                "3900",
                "--date",
                "2024-01-02",
            ],
            vec!["stockwise", "portfolio", "lots", "TCS"],
            vec![
                "stockwise",
                "portfolio",
                "lots",
                "TCS",
                "--lot",
                "1",
                "--date",
                "2022-01-03",
            ],
            vec!["stockwise", "portfolio", "actions"],
            vec!["stockwise", "portfolio", "actions", "RELIANCE", "--apply"],
            vec!["stockwise", "portfolio", "list"],
            vec!["stockwise", "portfolio", "--base", "usd"],
            vec!["stockwise", "portfolio", "show", "--base", "INR"],
            vec!["stockwise", "--portfolio", "long-term", "portfolio"],
            vec!["stockwise", "tax", "in", "--portfolio", "us"],
            vec!["stockwise", "import", "csv", "tradebook.csv", "--replace"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        // --lot and --date only make sense together.
        assert!(
            Cli::try_parse_from(["stockwise", "portfolio", "lots", "TCS", "--lot", "1"]).is_err()
        );
        assert!(Cli::try_parse_from(["stockwise", "portfolio", "--base", "eur"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "--portfolio", "../x", "portfolio"]).is_err());
    }

    #[test]
    fn planning_commands_parse() {
        for args in [
            vec!["stockwise", "tax", "export"],
            vec![
                "stockwise",
                "tax",
                "export",
                "in",
                "--fy",
                "2025-26",
                "--out",
                "cg.csv",
            ],
            vec!["stockwise", "tax", "export", "us", "--fy", "2025", "--json"],
            vec![
                "stockwise",
                "tax",
                "export",
                "India",
                "--portfolio",
                "long-term",
            ],
            vec!["stockwise", "portfolio", "benchmark"],
            vec![
                "stockwise",
                "portfolio",
                "benchmark",
                "SENSEX",
                "-p",
                "2y",
                "--json",
            ],
            vec![
                "stockwise",
                "p",
                "benchmark",
                "SP500",
                "--base",
                "inr",
                "--csv",
            ],
            vec![
                "stockwise",
                "goal",
                "sip",
                "--target",
                "1cr",
                "--years",
                "15",
            ],
            vec![
                "stockwise",
                "goal",
                "sip",
                "--target",
                "50L",
                "--years",
                "10",
                "--return",
                "11.5",
                "--step-up",
                "10",
                "--savings",
                "2L",
                "--json",
            ],
            vec![
                "stockwise",
                "goal",
                "retire",
                "--monthly-expense",
                "80000",
                "--years-to-retire",
                "20",
                "--years-in-retirement",
                "30",
                "--inflation",
                "6",
                "--return-pre",
                "12",
                "--return-post",
                "7",
            ],
            vec![
                "stockwise",
                "goal",
                "education",
                "--cost",
                "25L",
                "--years",
                "10",
                "--inflation",
                "8",
                "--json",
            ],
        ] {
            assert!(parses(&args), "{args:?} should parse");
        }
        for args in [
            vec!["stockwise", "tax"],
            vec!["stockwise", "tax", "export", "uk"],
            vec!["stockwise", "tax", "export", "in", "--csv"],
            vec!["stockwise", "portfolio", "benchmark", "-p", "5d"],
            vec!["stockwise", "portfolio", "--json", "benchmark"],
            vec!["stockwise", "goal"],
            vec!["stockwise", "goal", "sip", "--target", "1cr"],
            vec![
                "stockwise",
                "goal",
                "sip",
                "--target",
                "lots",
                "--years",
                "15",
            ],
            vec![
                "stockwise",
                "goal",
                "sip",
                "--target",
                "1cr",
                "--years",
                "0",
            ],
            vec![
                "stockwise",
                "goal",
                "sip",
                "--target",
                "1cr",
                "--years",
                "15",
                "--return",
                "80",
            ],
            vec![
                "stockwise",
                "goal",
                "sip",
                "--target",
                "1cr",
                "--years",
                "15",
                "--csv",
            ],
        ] {
            assert!(!parses(&args), "{args:?} should be rejected");
        }

        let cli = Cli::try_parse_from([
            "stockwise",
            "goal",
            "sip",
            "--target",
            "1cr",
            "--years",
            "15",
        ])
        .unwrap();
        let Commands::Goal {
            action:
                GoalCmd::Sip {
                    target,
                    years,
                    return_pct,
                    sip,
                },
        } = cli.command
        else {
            panic!("expected goal sip");
        };
        assert_eq!((target, years, return_pct), (1e7, 15, 12.0));
        assert_eq!(sip.step_up, 0.0);
        assert!(sip.savings.is_none() && !sip.out.json);

        let cli = Cli::try_parse_from(["stockwise", "tax", "export", "--fy", "2025-26"]).unwrap();
        let Commands::Tax {
            action: Some(TaxCmd::Export { country, fy, .. }),
            ..
        } = cli.command
        else {
            panic!("expected tax export");
        };
        assert!(country.is_none());
        assert_eq!(fy.as_deref(), Some("2025-26"));
    }

    #[test]
    fn portfolio_name_flag_is_normalized() {
        let cli = Cli::try_parse_from(["stockwise", "--portfolio", "Long-Term", "doctor"]).unwrap();
        assert_eq!(cli.portfolio.as_deref(), Some("long-term"));
    }

    #[test]
    fn interface_flags_parse() {
        assert!(Cli::try_parse_from(["stockwise", "tui"]).is_ok());
        assert!(Cli::try_parse_from(["stockwise", "tui", "-i", "60"]).is_ok());
        assert!(Cli::try_parse_from(["stockwise", "tui", "-i", "1"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "serve"]).is_ok());
        assert!(Cli::try_parse_from(["stockwise", "serve", "--open"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "stockwise",
                "serve",
                "--bind",
                "0.0.0.0:9000",
                "--allow-remote"
            ])
            .is_ok()
        );
    }

    #[test]
    fn typos_in_actions_are_rejected() {
        assert!(Cli::try_parse_from(["stockwise", "portfolio", "ad", "TCS", "1", "1"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "screen", "growht"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "trade", "by", "TCS", "1"]).is_err());
    }

    fn parses(args: &[&str]) -> bool {
        Cli::try_parse_from(args).is_ok()
    }

    #[test]
    fn scans_take_an_nse_universe() {
        for args in [
            vec!["stockwise", "movers", "--universe", "nifty50"],
            vec!["stockwise", "movers", "--universe", "NIFTY100", "--json"],
            vec!["stockwise", "heatmap", "--universe", "banknifty"],
            vec!["stockwise", "heatmap"],
            vec!["stockwise", "screen", "momentum", "--universe", "next50"],
            vec![
                "stockwise",
                "screen",
                "bluechip",
                "--universe",
                "niftyit",
                "--csv",
            ],
        ] {
            assert!(parses(&args), "{:?}", args);
        }
        assert!(!parses(&["stockwise", "movers", "--universe", "sensex"]));
        match Cli::try_parse_from(["stockwise", "movers", "--universe", "banknifty"])
            .unwrap()
            .command
        {
            Commands::Movers { universe, .. } => {
                assert_eq!(universe, Some(market::Universe::BankNifty))
            }
            _ => panic!("expected movers"),
        }
    }

    #[test]
    fn json_and_csv_flags_parse_where_supported() {
        for args in [
            vec!["stockwise", "quote", "TCS", "--csv"],
            vec!["stockwise", "portfolio", "--json"],
            vec!["stockwise", "portfolio", "--csv"],
            vec!["stockwise", "portfolio", "show", "--csv"],
            vec!["stockwise", "p", "show", "--json"],
            vec!["stockwise", "portfolio", "--base", "usd", "--json"],
            vec!["stockwise", "watch", "--csv"],
            vec!["stockwise", "watch", "show", "--json"],
            vec!["stockwise", "alert", "--json"],
            vec!["stockwise", "alert", "list", "--csv"],
            vec!["stockwise", "alert", "check", "--json"],
            vec!["stockwise", "technical", "TCS", "--json"],
            vec!["stockwise", "history", "TCS", "-p", "6mo", "--csv"],
            vec!["stockwise", "risk", "TCS", "--json"],
            vec!["stockwise", "backtest", "TCS", "-s", "macd", "--csv"],
            vec!["stockwise", "backtest", "TCS", "--json"],
            vec!["stockwise", "screen", "growth", "--csv"],
            vec!["stockwise", "movers", "--csv"],
            vec!["stockwise", "sectors", "--json"],
            vec!["stockwise", "compare", "TCS", "INFY", "--csv"],
            vec!["stockwise", "correlate", "TCS", "INFY", "--csv"],
            vec!["stockwise", "sip", "TCS", "5000", "--json"],
            vec![
                "stockwise",
                "whatif",
                "TCS",
                "100000",
                "2024-01-15",
                "--json",
            ],
            vec!["stockwise", "returns", "TCS", "--csv"],
            vec!["stockwise", "dividends", "ITC", "--json"],
            vec!["stockwise", "tax", "in", "--csv"],
            vec!["stockwise", "sim", "history", "--json"],
            vec!["stockwise", "config", "show", "--json"],
        ] {
            assert!(parses(&args), "{args:?} should parse");
        }
    }

    #[test]
    fn json_and_csv_are_mutually_exclusive() {
        for args in [
            vec!["stockwise", "quote", "TCS", "--json", "--csv"],
            vec!["stockwise", "portfolio", "--json", "--csv"],
            vec!["stockwise", "history", "TCS", "--csv", "--json"],
            vec!["stockwise", "backtest", "TCS", "--json", "--csv"],
        ] {
            assert!(!parses(&args), "{args:?} should be rejected");
        }
    }

    #[test]
    fn output_flags_only_where_they_make_sense() {
        // CSV only for tables; no output flags on actions that change data.
        assert!(!parses(&["stockwise", "technical", "TCS", "--csv"]));
        assert!(!parses(&["stockwise", "risk", "TCS", "--csv"]));
        assert!(!parses(&[
            "stockwise",
            "portfolio",
            "add",
            "TCS",
            "1",
            "1",
            "--json"
        ]));
        assert!(!parses(&[
            "stockwise",
            "portfolio",
            "--json",
            "add",
            "TCS",
            "1",
            "1"
        ]));
        assert!(!parses(&["stockwise", "watch", "add", "TCS", "--csv"]));
    }

    #[test]
    fn output_flags_map_to_formats() {
        let cli = Cli::try_parse_from(["stockwise", "portfolio", "show", "--csv"]).unwrap();
        let Commands::Portfolio {
            action: Some(PortfolioCmd::Show { out: show }),
            out,
            ..
        } = cli.command
        else {
            panic!("expected portfolio show");
        };
        assert_eq!(out.or(show), Format::Csv);
        assert_eq!(Format::from(JsonOutput { json: true }), Format::Json);
        assert_eq!(Format::from(TableOutput::default()), Format::Table);
    }

    #[test]
    fn config_subcommands_parse() {
        assert!(parses(&["stockwise", "config"]));
        assert!(parses(&["stockwise", "config", "path"]));
        assert!(parses(&["stockwise", "config", "init", "--force"]));
        assert!(!parses(&["stockwise", "config", "path", "--json"]));
    }

    fn with_config(text: &str) -> clap::Command {
        let cmd = build_cli();
        let names: Vec<String> = period_defaults(&cmd).into_iter().map(|(c, _)| c).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let cfg = config::parse(text, &refs).unwrap();
        with_config_defaults(cmd, &cfg)
    }

    fn matches(cmd: clap::Command, args: &[&str]) -> Cli {
        Cli::from_arg_matches(&cmd.try_get_matches_from(args).unwrap()).unwrap()
    }

    #[test]
    fn config_file_sets_defaults_and_flags_still_win() {
        let cfg = "market = \"us\"\nai = true\n[periods]\ntechnical = \"6mo\"\n";

        let cli = matches(with_config(cfg), &["stockwise", "technical", "AAPL"]);
        assert_eq!(cli.market, Market::Us, "config beats the built-in default");
        assert!(cli.ai && !cli.no_ai);
        let Commands::Technical { period, .. } = cli.command else {
            panic!("expected technical");
        };
        assert_eq!(period, "6mo");

        let cli = matches(
            with_config(cfg),
            &[
                "stockwise",
                "-m",
                "in",
                "--no-ai",
                "technical",
                "TCS",
                "-p",
                "1y",
            ],
        );
        assert_eq!(cli.market, Market::In, "a flag beats the config file");
        assert!(cli.no_ai);
        let Commands::Technical { period, .. } = cli.command else {
            panic!("expected technical");
        };
        assert_eq!(period, "1y");

        // Other commands keep their built-in default period.
        let cli = matches(with_config(cfg), &["stockwise", "risk", "TCS"]);
        let Commands::Risk { period, .. } = cli.command else {
            panic!("expected risk");
        };
        assert_eq!(period, "1y");
    }

    #[test]
    fn help_shows_the_config_default() {
        let mut cmd = with_config("[periods]\nbacktest = \"5y\"\n");
        let help = cmd
            .find_subcommand_mut("backtest")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(help.contains("[default: 5y]"), "{help}");
    }

    #[test]
    fn period_defaults_cover_every_period_command() {
        let names: Vec<String> = period_defaults(&build_cli())
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        for cmd in ["technical", "history", "risk", "backtest", "sip", "deep"] {
            assert!(names.iter().any(|n| n == cmd), "{cmd} takes --period");
        }
        assert!(!names.iter().any(|n| n == "quote"));
    }

    fn options_cmd(args: &[&str]) -> (Option<OptionsCmd>, Option<String>, String) {
        let cli = Cli::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        match cli.command {
            Commands::Options {
                action,
                symbol,
                period,
            } => (action, symbol, period),
            _ => panic!("not the options command"),
        }
    }

    #[test]
    fn options_keeps_the_bare_expected_moves_form() {
        let (action, symbol, period) = options_cmd(&["stockwise", "options", "TCS", "-p", "3mo"]);
        assert!(action.is_none());
        assert_eq!(symbol.as_deref(), Some("TCS"));
        assert_eq!(period, "3mo");

        let (action, ..) = options_cmd(&["stockwise", "options", "moves", "TCS"]);
        assert!(matches!(action, Some(OptionsCmd::Moves { symbol, .. }) if symbol == "TCS"));

        let (action, ..) = options_cmd(&[
            "stockwise",
            "options",
            "chain",
            "NIFTY",
            "-e",
            "2026-10-27",
            "-n",
            "5",
        ]);
        match action {
            Some(OptionsCmd::Chain {
                symbol,
                expiry,
                strikes,
                ..
            }) => {
                assert_eq!(symbol, "NIFTY");
                assert_eq!(expiry, chrono::NaiveDate::from_ymd_opt(2026, 10, 27));
                assert_eq!(strikes, 5);
            }
            _ => panic!("expected options chain"),
        }
        let (action, ..) = options_cmd(&["stockwise", "options", "max-pain", "NIFTY"]);
        assert!(matches!(action, Some(OptionsCmd::Maxpain { .. })));
        let (action, ..) = options_cmd(&["stockwise", "options", "pcr", "AAPL", "--json"]);
        assert!(matches!(action, Some(OptionsCmd::Pcr { json: true, .. })));

        let (action, ..) = options_cmd(&[
            "stockwise",
            "options",
            "payoff",
            "NIFTY",
            "--leg",
            "buy call 24500",
            "--leg",
            "sell call 24800 @ 40",
            "--lots",
            "2",
            "--today",
        ]);
        match action {
            Some(OptionsCmd::Payoff {
                symbol,
                legs,
                strategy,
                lots,
                today,
                ..
            }) => {
                assert_eq!(symbol, "NIFTY");
                assert_eq!(legs.len(), 2);
                assert_eq!(legs[1].price, Some(40.0));
                assert!(strategy.is_none());
                assert_eq!(lots, 2);
                assert!(today);
            }
            _ => panic!("expected options payoff"),
        }
        let (action, ..) = options_cmd(&[
            "stockwise",
            "options",
            "payoff",
            "SENSEX",
            "--strategy",
            "iron-condor",
            "-w",
            "500",
        ]);
        assert!(matches!(
            action,
            Some(OptionsCmd::Payoff {
                strategy: Some(StrategyArg::IronCondor),
                width: Some(w),
                ..
            }) if w == 500.0
        ));
        for bad in [
            vec![
                "stockwise",
                "options",
                "payoff",
                "NIFTY",
                "--leg",
                "hold call 100",
            ],
            vec![
                "stockwise",
                "options",
                "payoff",
                "NIFTY",
                "--strategy",
                "condor",
            ],
            vec!["stockwise", "options", "payoff", "NIFTY", "-e", "27/10"],
            vec![
                "stockwise",
                "options",
                "payoff",
                "NIFTY",
                "-s",
                "straddle",
                "--lot-size",
                "0",
            ],
        ] {
            assert!(Cli::try_parse_from(&bad).is_err(), "{bad:?}");
        }

        assert!(Cli::try_parse_from(["stockwise", "options"]).is_err());
        assert!(
            Cli::try_parse_from(["stockwise", "options", "chain", "NIFTY", "-e", "27/10"]).is_err()
        );
    }

    #[test]
    fn mf_commands_parse() {
        for args in [
            vec!["stockwise", "mf", "search", "parag", "parikh", "flexi"],
            vec!["stockwise", "mf", "nav", "122639", "120465", "--json"],
            vec!["stockwise", "mf", "history", "122639", "-p", "3y"],
            vec!["stockwise", "mf", "sip", "122639", "5000", "-p", "max"],
            vec!["stockwise", "mf", "screen"],
            vec![
                "stockwise",
                "mf",
                "screen",
                "flexi",
                "cap",
                "--sort",
                "5y",
                "--plan",
                "all",
            ],
            vec!["stockwise", "mf", "overlap", "SPY", "QQQ", "-m", "us"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        assert!(Cli::try_parse_from(["stockwise", "mf", "nav", "abc"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "mf", "history", "1", "-p", "3x"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "mf", "screen", "--sort", "2y"]).is_err());
        assert!(Cli::try_parse_from(["stockwise", "mf"]).is_err());

        let cli = Cli::try_parse_from([
            "stockwise",
            "mf",
            "screen",
            "elss",
            "--sort",
            "ter",
            "--max-ter",
            "0.9%",
        ])
        .unwrap();
        match cli.command {
            Commands::Mf {
                action: MfCmd::Screen { sort, max_ter, .. },
            } => {
                assert!(matches!(sort, MfSort::Ter));
                assert_eq!(max_ter, Some(0.9));
            }
            _ => panic!("expected mf screen"),
        }
        for bad in ["0", "-1", "9", "x"] {
            assert!(
                Cli::try_parse_from(["stockwise", "mf", "screen", "elss", "--max-ter", bad])
                    .is_err(),
                "--max-ter {bad}"
            );
        }
    }

    #[test]
    fn trade_broker_flag_and_order_types_parse() {
        for args in [
            vec!["stockwise", "trade", "--broker", "zerodha", "holdings"],
            vec!["stockwise", "trade", "holdings", "--broker", "kite"],
            vec!["stockwise", "trade", "orders", "--broker", "Upstox"],
            vec!["stockwise", "trade", "login", "--broker", "fyers"],
            vec![
                "stockwise",
                "trade",
                "config",
                "--keyring",
                "--broker",
                "dhan",
            ],
            vec![
                "stockwise",
                "trade",
                "sell",
                "TCS",
                "5",
                "--trigger",
                "3400",
            ],
            vec![
                "stockwise",
                "trade",
                "sell",
                "TCS",
                "5",
                "--trigger",
                "3400",
                "--limit",
                "3390",
            ],
            vec![
                "stockwise",
                "trade",
                "buy",
                "TCS",
                "5",
                "--limit",
                "3300",
                "--dry-run",
            ],
            vec![
                "stockwise",
                "trade",
                "gtt",
                "sell",
                "TCS",
                "5",
                "--trigger",
                "3400",
                "--limit",
                "3390",
            ],
            vec!["stockwise", "trade", "gtt", "list"],
            vec!["stockwise", "trade", "gtt", "cancel", "12345", "--yes"],
            vec!["stockwise", "import", "groww", "h.csv"],
            vec!["stockwise", "import", "upstox", "h.csv"],
            vec!["stockwise", "import", "cas", "cas.json"],
            vec!["stockwise", "import", "cams", "cas.json"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        // GTT needs both prices; unknown brokers are rejected.
        assert!(
            Cli::try_parse_from([
                "stockwise",
                "trade",
                "gtt",
                "sell",
                "TCS",
                "5",
                "--trigger",
                "1"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["stockwise", "trade", "--broker", "robinhood", "holdings"])
                .is_err()
        );
    }

    #[test]
    fn trade_selftest_sync_and_gtt_list_flags_parse() {
        for args in [
            vec!["stockwise", "trade", "selftest"],
            vec![
                "stockwise",
                "trade",
                "selftest",
                "--broker",
                "dhan",
                "--json",
            ],
            vec!["stockwise", "trade", "selftest", "--redact-amounts"],
            vec!["stockwise", "trade", "sync", "--broker", "zerodha"],
            vec![
                "stockwise",
                "trade",
                "sync",
                "--since",
                "2026-10-01",
                "--apply",
            ],
            vec![
                "stockwise",
                "trade",
                "sync",
                "--apply",
                "--notify",
                "--json",
            ],
            vec!["stockwise", "trade", "gtt", "list", "--notify", "--json"],
            vec!["stockwise", "trade", "gtt", "show"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} should parse");
        }
        // Selftest never takes order flags.
        assert!(Cli::try_parse_from(["stockwise", "trade", "selftest", "--yes"]).is_err());
        let cli =
            Cli::try_parse_from(["stockwise", "trade", "sync", "--since", "2026-10-01"]).unwrap();
        let Commands::Trade {
            action:
                TradeCmd::Sync {
                    since,
                    apply,
                    notify,
                    ..
                },
            ..
        } = cli.command
        else {
            panic!("expected trade sync");
        };
        assert_eq!(since.as_deref(), Some("2026-10-01"));
        assert!(!apply && !notify, "sync previews by default");
    }

    #[test]
    fn trade_defaults_to_angel() {
        // SAFETY of the default: an unset STOCKWISE_BROKER keeps 0.3 behaviour.
        if std::env::var_os("STOCKWISE_BROKER").is_some() {
            return;
        }
        let cli = Cli::try_parse_from(["stockwise", "trade", "holdings"]).unwrap();
        let Commands::Trade { broker, .. } = cli.command else {
            panic!("expected trade");
        };
        assert_eq!(broker, brokers::BrokerKind::Angel);
    }

    #[test]
    fn value_names_match_handler_strings() {
        assert_eq!(value_name(CostsArg::InDelivery), "in-delivery");
        assert_eq!(value_name(FillArg::NextOpen), "next-open");
        assert_eq!(value_name(ImportSource::Kite), "kite");
        assert_eq!(value_name(ImportSource::Groww), "groww");
        assert_eq!(value_name(ImportSource::Cas), "cas");
        assert_eq!(value_name(TaxCountry::In), "in");
        assert_eq!(value_name(AlertWhen::Above), "above");
        assert_eq!(value_name(DaemonMode::Longterm), "longterm");
    }
}
