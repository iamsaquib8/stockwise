#[allow(dead_code)]
mod alerts;
mod api;
mod backtest;
mod charts;
mod commands;
mod display;
mod insights;
mod ai;
mod angel;
mod daemon;
mod intraday;
mod longterm;
mod market;
mod simulator;
mod portfolio;
mod technical;
mod watchlist;
mod wealth;

use clap::{Parser, Subcommand};
use market::Market;

#[derive(Parser)]
#[command(
    name = "stockwise",
    version,
    about = "Autonomous stock market wealth generation CLI",
    long_about = "StockWise — Your terminal-based stock market companion.\n\n\
    Real-time quotes, deep analysis, backtesting, SIP simulation,\n\
    price forecasting, portfolio optimization, and more.\n\n\
    Supports both US and Indian (NSE/BSE) markets.\n\
    Use --market in for Indian stocks (e.g. RELIANCE → RELIANCE.NS)"
)]
struct Cli {
    /// Market: us (default) or in (India — auto-appends .NS)
    #[arg(short = 'm', long = "market", global = true, default_value = "in")]
    market: Market,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    // ── Core ──
    /// Get a real-time quote for one or more stocks
    #[command(alias = "q")]
    Quote { symbols: Vec<String> },

    /// Deep fundamental analysis with AI-powered insights
    #[command(alias = "a")]
    Analyze { symbol: String },

    /// Technical analysis (RSI, MACD, Bollinger Bands, etc.)
    #[command(alias = "t")]
    Technical {
        symbol: String,
        #[arg(short, long, default_value = "3mo")]
        period: String,
    },

    /// Compare multiple stocks side-by-side
    #[command(alias = "c")]
    Compare { symbols: Vec<String> },

    /// View historical price data with charts
    /// View historical price data with charts
    History {
        symbol: String,
        /// Period: 1d, 5d, 1w, 1mo, 3mo, 6mo, 1y, 2y, 5y, max
        #[arg(short, long, default_value = "1mo")]
        period: String,
    },

    /// Multi-timeframe charts (1W, 1M, 3M, 1Y at once)
    Chart {
        symbol: String,
    },

    // ── Portfolio & Tracking ──
    /// Manage your stock portfolio
    #[command(alias = "p")]
    Portfolio {
        action: String,
        symbol: Option<String>,
        shares: Option<f64>,
        cost: Option<f64>,
    },

    /// Manage your watchlist
    #[command(alias = "w")]
    Watch {
        #[arg(default_value = "show")]
        action: String,
        symbols: Vec<String>,
    },

    /// Price alerts (add/remove/check/list)
    Alert {
        #[arg(default_value = "list")]
        action: String,
        symbol: Option<String>,
        condition: Option<String>,
        target: Option<f64>,
    },

    /// Export portfolio or watchlist to CSV
    Export { what: String },

    // ── Market Intelligence ──
    /// Show major market indices (US + India)
    Markets,

    /// Top gainers and losers of the day
    Movers,

    /// Sector performance overview with visual bars
    Sectors,

    /// Recent news headlines for a stock
    News { symbol: String },

    /// Search for a stock symbol by name
    Search { query: String },

    /// Market sentiment — Fear & Greed gauge
    Sentiment,

    // ── Analysis ──
    /// Risk analysis (volatility, Sharpe, drawdown, VaR)
    Risk {
        symbol: String,
        #[arg(short, long, default_value = "1mo")]
        period: String,
    },

    /// Price correlation matrix between stocks
    #[command(alias = "corr")]
    Correlate { symbols: Vec<String> },

    /// Screen stocks: undervalued, growth, dividend, momentum, bluechip
    Screen { category: String },

    /// Multi-timeframe analysis — all timeframes at a glance
    #[command(alias = "mtf")]
    Timeframes { symbol: String },

    /// AI-scored top investment picks
    Picks,

    /// Earnings overview for portfolio & watchlist stocks
    Earnings,

    // ── Wealth Generation ──
    /// Backtest trading strategies on historical data
    Backtest {
        /// Stock symbol
        symbol: String,
        /// Strategy: rsi, macd, sma, bb, vwap, mr
        #[arg(short, long, default_value = "rsi")]
        strategy: String,
        /// Period: 1y, 2y, 5y
        #[arg(short, long, default_value = "2y")]
        period: String,
    },

    /// SIP/DCA simulator — what if you invested X/month
    Sip {
        /// Stock symbol
        symbol: String,
        /// Monthly investment amount
        amount: f64,
        /// Period: 1y, 2y, 5y, 10y
        #[arg(short, long, default_value = "5y")]
        period: String,
    },

    /// Price forecast with trend projection & confidence bands
    Forecast {
        symbol: String,
        /// Days to project forward
        #[arg(short, long, default_value = "30")]
        days: usize,
        /// Historical data period for trend calculation
        #[arg(long, default_value = "1mo")]
        period: String,
    },

    /// Intraday bot: suggest trades for target return
    #[command(alias = "bot")]
    Intraday { amount: f64, target: f64 },

    /// Everything about a stock in one command
    Deep {
        symbol: String,
        #[arg(short, long, default_value = "3mo")]
        period: String,
    },

    /// Sector rotation — which sectors to overweight/underweight
    Rotation,
    /// Quality vs Growth matrix — categorize stocks into quadrants
    Matrix,
    /// Earnings momentum — accelerating vs decelerating earnings
    Surprise,
    /// Dividend dashboard for portfolio + watchlist
    Divcal,
    /// Portfolio correlation dashboard — find redundant holdings
    Pcorr,
    /// Returns across all timeframes (1W to 5Y)
    Returns { symbol: String },
    /// Market heatmap — visual overview of all stocks
    Heatmap,
    /// Candlestick pattern detection
    Patterns {
        symbol: String,
        #[arg(short, long, default_value = "1mo")]
        period: String,
    },
    /// Compare two sectors head-to-head
    Sectorcmp { sector1: String, sector2: String },
    /// What-if: if you invested X on a past date
    Whatif { symbol: String, amount: f64, date: String },

    /// Run continuous market daemon (intraday worker or longterm daily)
    Daemon {
        /// Mode: intraday, longterm
        mode: String,
        /// Capital for intraday (default 25000)
        amount: Option<f64>,
    },

    /// Paper trading simulator — test bot without real money
    Sim {
        /// Action: start, status, settle, history, reset
        #[arg(default_value = "help")]
        action: String,
        /// Capital amount (for start)
        amount: Option<f64>,
        /// Target % (for start)
        target: Option<f64>,
    },

    /// AI-powered reports (uses local Ollama)
    Report {
        /// Type: intraday, longterm, portfolio, or a stock symbol
        what: String,
    },

    /// Track portfolio value over time (run daily)
    Wealth,

    /// Suggest trades to rebalance portfolio (equal-weight)
    Rebalance,

    /// Find tax-loss harvesting opportunities in portfolio
    Harvest,

    /// Import holdings from Kite/IndMoney/CSV
    Import {
        /// Source: kite, indmoney, csv
        source: String,
        /// Path to exported CSV file
        file: String,
    },

    /// Long-term wealth generation bot — best stocks for SIP
    Longterm {
        /// Monthly SIP budget (default 10000)
        amount: Option<f64>,
    },

    /// Tax calculator based on country (in/us/uk)
    Tax {
        /// Country: in (India), us (USA), uk (UK)
        country: String,
    },

    // ── Advanced Stock Features ──
    /// Support & resistance levels (pivot points + Fibonacci)
    Support {
        symbol: String,
        #[arg(short, long, default_value = "1mo")]
        period: String,
    },

    /// Volume profile analysis (OBV, A/D line, volume stats)
    #[command(alias = "vol")]
    Volume {
        symbol: String,
        #[arg(short, long, default_value = "1mo")]
        period: String,
    },

    /// Detect price gaps (unfilled/filled)
    Gaps {
        symbol: String,
        #[arg(short, long, default_value = "1mo")]
        period: String,
    },

    /// Stop-loss calculator (ATR-based, percentage, Chandelier exit)
    Stoploss {
        symbol: String,
        #[arg(short, long)]
        entry: Option<f64>,
        #[arg(long, default_value = "1mo")]
        period: String,
    },

    /// Compare against industry peers automatically
    Peers { symbol: String },

    /// Dividend analysis with income projection and DRIP growth
    Dividends { symbol: String },

    /// Insider activity and analyst consensus
    Insider { symbol: String },

    /// IPO calendar and news
    Ipo,

    /// Options overview (expected moves, strike suggestions)
    Options {
        symbol: String,
        #[arg(short, long, default_value = "1mo")]
        period: String,
    },

    /// Fibonacci retracement levels across multiple timeframes
    Fibs {
        symbol: String,
        #[arg(short, long, default_value = "1mo")]
        period: String,
    },

    /// Angel One trading: setup, buy, sell, holdings, orders
    Trade {
        /// Action: setup, config, holdings, positions, buy, sell, limit, orders
        action: String,
        /// Symbol (for buy/sell/limit)
        symbol: Option<String>,
        /// Quantity (for buy/sell/limit)
        qty: Option<u32>,
        /// Price (for limit orders)
        price: Option<f64>,
    },

    /// Combined dashboard: markets + portfolio + watchlist + alerts
    #[command(alias = "d")]
    Dashboard,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let m = cli.market;

    let result = match cli.command {
        Commands::Quote { symbols } => commands::cmd_quote(&symbols, m).await,
        Commands::Analyze { symbol } => commands::cmd_analyze(&symbol, m).await,
        Commands::Technical { symbol, period } => commands::cmd_technical(&symbol, &period, m).await,
        Commands::Compare { symbols } => commands::cmd_compare(&symbols, m).await,
        Commands::History { symbol, period } => commands::cmd_history(&symbol, &period, m).await,
        Commands::Chart { symbol } => commands::cmd_chart(&symbol, m).await,
        Commands::Portfolio { action, symbol, shares, cost } => {
            commands::cmd_portfolio(&action, symbol.as_deref(), shares, cost, m).await
        }
        Commands::Watch { action, symbols } => commands::cmd_watchlist(&action, &symbols, m).await,
        Commands::Alert { action, symbol, condition, target } => {
            commands::cmd_alert(&action, symbol.as_deref(), condition.as_deref(), target, m).await
        }
        Commands::Export { what } => commands::cmd_export(&what, m).await,
        Commands::Markets => commands::cmd_markets(m).await,
        Commands::Movers => commands::cmd_movers(m).await,
        Commands::Sectors => commands::cmd_sectors(m).await,
        Commands::News { symbol } => commands::cmd_news(&symbol, m).await,
        Commands::Search { query } => commands::cmd_search(&query).await,
        Commands::Sentiment => commands::cmd_sentiment().await,
        Commands::Risk { symbol, period } => commands::cmd_risk(&symbol, &period, m).await,
        Commands::Correlate { symbols } => commands::cmd_correlate(&symbols, m).await,
        Commands::Screen { category } => commands::cmd_screen(&category, m).await,
        Commands::Timeframes { symbol } => commands::cmd_multitimeframe(&symbol, m).await,
        Commands::Picks => commands::cmd_picks(m).await,
        Commands::Earnings => commands::cmd_earnings(m).await,
        Commands::Backtest { symbol, strategy, period } => {
            commands::cmd_backtest(&symbol, &strategy, &period, m).await
        }
        Commands::Sip { symbol, amount, period } => {
            commands::cmd_sip(&symbol, amount, &period, m).await
        }
        Commands::Forecast { symbol, days, period } => commands::cmd_forecast(&symbol, days, &period, m).await,
        Commands::Deep { symbol, period } => commands::cmd_deep(&symbol, &period, m).await,
        Commands::Rotation => commands::cmd_rotation(m).await,
        Commands::Matrix => commands::cmd_matrix(m).await,
        Commands::Surprise => commands::cmd_surprise(m).await,
        Commands::Divcal => commands::cmd_divcal(m).await,
        Commands::Pcorr => commands::cmd_pcorr(m).await,
        Commands::Returns { symbol } => commands::cmd_returns(&symbol, m).await,
        Commands::Heatmap => commands::cmd_heatmap(m).await,
        Commands::Patterns { symbol, period } => commands::cmd_patterns(&symbol, &period, m).await,
        Commands::Sectorcmp { sector1, sector2 } => commands::cmd_sectorcmp(&sector1, &sector2, m).await,
        Commands::Whatif { symbol, amount, date } => commands::cmd_whatif(&symbol, amount, &date, m).await,
        Commands::Daemon { mode, amount } => commands::cmd_daemon(&mode, amount, m).await,
        Commands::Intraday { amount, target } => commands::cmd_intraday(amount, target, m).await,
        Commands::Sim { action, amount, target } => commands::cmd_sim(&action, amount, target, m).await,
        Commands::Report { what } => commands::cmd_report(&what, m).await,
        Commands::Wealth => commands::cmd_wealth(m).await,
        Commands::Rebalance => commands::cmd_rebalance(&[], m).await,
        Commands::Harvest => commands::cmd_harvest(m).await,
        Commands::Import { source, file } => commands::cmd_import(&source, &file, m).await,
        Commands::Longterm { amount } => commands::cmd_longterm(amount, m).await,
        Commands::Tax { country } => commands::cmd_tax(&country, m).await,
        Commands::Support { symbol, period } => commands::cmd_support(&symbol, &period, m).await,
        Commands::Volume { symbol, period } => commands::cmd_volume(&symbol, &period, m).await,
        Commands::Gaps { symbol, period } => commands::cmd_gaps(&symbol, &period, m).await,
        Commands::Stoploss { symbol, entry, period } => commands::cmd_stoploss(&symbol, entry, &period, m).await,
        Commands::Peers { symbol } => commands::cmd_peers(&symbol, m).await,
        Commands::Dividends { symbol } => commands::cmd_dividends(&symbol, m).await,
        Commands::Insider { symbol } => commands::cmd_insider(&symbol, m).await,
        Commands::Ipo => commands::cmd_ipo().await,
        Commands::Options { symbol, period } => commands::cmd_options(&symbol, &period, m).await,
        Commands::Fibs { symbol, period } => commands::cmd_fibs(&symbol, &period, m).await,
        Commands::Trade { action, symbol, qty, price } => {
            commands::cmd_trade(&action, symbol.as_deref(), qty, price, m).await
        }
        Commands::Dashboard => commands::cmd_dashboard(m).await,
    };

    if let Err(e) = result {
        eprintln!("\n  {} {}\n", colored::Colorize::red("Error:"), e);
        std::process::exit(1);
    }
}
