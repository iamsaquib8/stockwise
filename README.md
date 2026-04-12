# StockWise

A powerful autonomous stock market CLI built in Rust. Real-time quotes, deep analysis, backtesting, SIP simulation, price forecasting, intraday signals, tax calculation, and wealth tracking — all from your terminal. Supports both **US** and **Indian (NSE/BSE)** markets.

**30+ commands** | **No API key needed** | **4.5MB binary** | **Braille charts in terminal**

## Install

```bash
cargo install --path .
```

Or build manually:

```bash
cargo build --release
# Binary at target/release/stockwise
```

## Quick Start

```bash
# US stocks (default)
stockwise quote AAPL MSFT NVDA
stockwise analyze AAPL
stockwise technical TSLA

# Indian stocks
stockwise -m in quote RELIANCE TCS INFY
stockwise -m in analyze HDFCBANK

# Market overview
stockwise markets
stockwise dashboard
stockwise sentiment
```

## All Commands

### Core Analysis

| Command | Alias | Description |
|---------|-------|-------------|
| `quote <SYMBOLS...>` | `q` | Live price, volume, P/E, market cap, moving averages |
| `analyze <SYMBOL>` | `a` | Deep fundamental analysis with AI-powered insights and verdict |
| `technical <SYMBOL>` | `t` | RSI, MACD, Bollinger Bands, ATR, VWAP with braille charts |
| `compare <SYMBOLS...>` | `c` | Side-by-side comparison table across 16 metrics |
| `history <SYMBOL> -p 1y` | | Historical prices with braille line charts + volume bars |

### Market Intelligence

| Command | Description |
|---------|-------------|
| `markets` | US + India major indices at a glance |
| `movers` | Top 10 gainers and losers from 30 popular stocks |
| `sectors` | Sector performance with visual bar charts |
| `news <SYMBOL>` | Latest 10 headlines from Yahoo Finance |
| `search <QUERY>` | Find stock symbols by company name |
| `sentiment` | Market Fear & Greed gauge (VIX + momentum + breadth) |

### Advanced Analysis

| Command | Alias | Description |
|---------|-------|-------------|
| `risk <SYMBOL>` | | Volatility, Sharpe/Sortino/Calmar ratios, max drawdown, VaR |
| `correlate <SYMBOLS...>` | `corr` | Correlation matrix from 1 year of daily returns |
| `screen <CATEGORY>` | | Filter stocks: `undervalued`, `growth`, `dividend`, `momentum`, `bluechip` |
| `timeframes <SYMBOL>` | `mtf` | Multi-timeframe analysis — 1M/3M/6M/1Y/2Y/5Y signals at a glance |
| `picks` | | AI-scored top investment picks combining 8 factors |
| `earnings` | | Earnings overview for portfolio & watchlist stocks |

### Wealth Generation

| Command | Description |
|---------|-------------|
| `backtest <SYM> -s rsi -p 2y` | Backtest strategies: `rsi`, `macd`, `sma`, `bb`, `vwap`, `mr` |
| `sip <SYM> <AMOUNT> -p 5y` | SIP/DCA simulator — what if you invested X/month |
| `forecast <SYM> -d 30` | Price forecast with trend projection & 95% confidence bands |
| `intraday <AMOUNT> <TARGET%>` | Intraday bot — scan NIFTY stocks, suggest trades for target return |
| `longterm [AMOUNT]` | Long-term wealth bot — best stocks for SIP with allocation plan |

### Portfolio & Tracking

| Command | Alias | Description |
|---------|-------|-------------|
| `portfolio add <SYM> <SHARES> <COST>` | `p` | Add a holding |
| `portfolio show` | `p` | View holdings with live P&L + allocation chart |
| `watch add <SYMBOLS...>` | `w` | Add to persistent watchlist |
| `watch` | `w` | View watchlist with live prices |
| `alert add <SYM> above/below <PRICE>` | | Set a price alert |
| `alert check` | | Check all alerts against live prices |
| `export portfolio` | | Export to CSV |
| `wealth` | | Track portfolio value over time (run daily for growth chart) |
| `rebalance` | | Suggest trades to rebalance to equal-weight |
| `harvest` | | Find tax-loss harvesting opportunities |
| `tax <COUNTRY>` | | Tax calculator — `in` (India), `us` (USA), `uk` (UK) |
| `dashboard` | `d` | Combined view: indices + portfolio + watchlist + alerts |

## Indian Market Support

Use `-m in` (or `--market in`) to auto-resolve symbols to NSE:

```bash
stockwise -m in quote RELIANCE TCS INFY
stockwise -m in longterm 5000          # Best stocks for SIP
stockwise -m in bot 10000 2            # Intraday: ₹10K capital, 2% target
stockwise -m in tax in                 # Indian tax calculation
```

- Prices display with `₹` symbol automatically
- Large numbers use lakhs/crores (e.g., `₹18.28L Cr`)
- BSE symbols work too: `stockwise quote RELIANCE.BO`

## Terminal Charts

StockWise renders rich visualizations directly in your terminal:

- **Braille line charts** — high-resolution price charts using Unicode braille characters
- **Dual-line overlays** — price vs SMA-20, portfolio value vs invested amount
- **Volume bars** — volume profile visualization
- **Bar charts** — sector performance, portfolio allocation
- **RSI gauge** — visual overbought/oversold indicator
- **Fear & Greed gauge** — market sentiment visualization

## Backtesting Strategies

Test 6 trading strategies on any stock:

```bash
stockwise backtest AAPL -s rsi -p 2y    # RSI: buy <30, sell >70
stockwise backtest AAPL -s macd -p 5y   # MACD crossover
stockwise backtest AAPL -s sma -p 2y    # SMA 20/50 crossover
stockwise backtest AAPL -s bb -p 2y     # Bollinger Band bounce
stockwise backtest AAPL -s vwap -p 1y   # VWAP crossover
stockwise backtest AAPL -s mr -p 2y     # Mean reversion (3% deviation)
```

Shows equity curve chart, win rate, Sharpe ratio, max drawdown, and alpha vs buy-and-hold.

## Technical Indicators

- **Moving Averages**: SMA (10, 20, 50, 100, 200), EMA (12, 26)
- **Momentum**: RSI (14), MACD (12/26/9) with histogram
- **Volatility**: Bollinger Bands (20,2), ATR (14), VWAP
- **Risk**: Annualized volatility, Sharpe/Sortino/Calmar ratios, VaR (95%/99%), max drawdown
- **Statistics**: Correlation matrix, linear regression, daily returns

## Data

- **Source**: Yahoo Finance (no API key required, auto cookie/crumb auth)
- **Storage**: Portfolio, watchlist, alerts, and wealth history stored as JSON in:
  - macOS: `~/Library/Application Support/stockwise/`
  - Linux: `~/.local/share/stockwise/`

## License

MIT
