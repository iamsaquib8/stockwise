# StockWise

An autonomous stock market CLI built in Rust. Real-time quotes, deep analysis, backtesting, SIP simulation, price forecasting, intraday/long-term bots, live trading via Angel One, tax calculation, and wealth tracking — all from your terminal.

Supports both **US** and **Indian (NSE/BSE)** markets. Default market is **India**.

**35+ commands** | **Live trading** | **No API key for data** | **Braille terminal charts**

---

## Install

```bash
# Build from source
cargo build --release

# Binary at target/release/stockwise
# Or install globally:
cargo install --path .
```

## Quick Start

```bash
# Indian stocks (default market)
stockwise quote RELIANCE TCS INFY
stockwise analyze HDFCBANK
stockwise technical TATAMOTORS

# US stocks
stockwise -m us quote AAPL MSFT NVDA
stockwise -m us analyze TSLA

# Market overview
stockwise markets
stockwise sentiment
stockwise dashboard
```

---

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
| `movers` | Top 10 gainers and losers from 30 large-cap stocks |
| `sectors` | Sector performance with visual bar charts |
| `news <SYMBOL>` | Latest 10 headlines from Yahoo Finance |
| `search <QUERY>` | Find stock symbols by company name |
| `sentiment` | Market Fear & Greed gauge (VIX, momentum, breadth) |

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

### Live Trading (Angel One)

| Command | Description |
|---------|-------------|
| `trade setup` | Setup instructions for Angel One SmartAPI |
| `trade config` | Create/edit API credentials config file |
| `trade holdings` | View live holdings from Angel One (auto-syncs to StockWise) |
| `trade positions` | View open intraday positions |
| `trade buy <SYM> <QTY>` | Place a market buy order |
| `trade sell <SYM> <QTY>` | Place a market sell order |
| `trade limit <SYM> <QTY> <PRICE>` | Place a limit buy order |
| `trade orders` | View today's order book |

### Portfolio & Tracking

| Command | Alias | Description |
|---------|-------|-------------|
| `portfolio add <SYM> <SHARES> <COST>` | `p` | Add a holding manually |
| `portfolio show` | `p` | View holdings with live P&L + allocation chart |
| `import kite <FILE>` | | Import holdings from Zerodha Kite CSV export |
| `import indmoney <FILE>` | | Import holdings from IndMoney CSV export |
| `import csv <FILE>` | | Import from any CSV with Symbol, Quantity, Price columns |
| `watch add <SYMBOLS...>` | `w` | Add to persistent watchlist |
| `watch` | `w` | View watchlist with live prices |
| `alert add <SYM> above/below <PRICE>` | | Set a price alert |
| `alert check` | | Check all alerts against live prices |
| `export portfolio` | | Export portfolio to CSV |
| `export watchlist` | | Export watchlist to CSV |
| `wealth` | | Snapshot portfolio value (run daily for growth chart) |
| `rebalance` | | Suggest trades to rebalance to equal-weight |
| `harvest` | | Find tax-loss harvesting opportunities |
| `tax <COUNTRY>` | | Tax calculator — `in` (India), `us` (USA), `uk` (UK) |
| `dashboard` | `d` | Combined view: indices + portfolio + watchlist + alerts |

---

## Angel One Live Trading

StockWise integrates with **Angel One SmartAPI** (free) for live order placement directly from terminal.

### Setup

1. Create an account at [Angel One](https://www.angelone.in/)
2. Register an app at [smartapi.angelone.in](https://smartapi.angelone.in/) to get your **API Key**
3. Enable TOTP on your Angel One account and note the **TOTP secret** (base32 string shown during setup)

```bash
# Create the config file
stockwise trade config

# Edit with your credentials:
# ~/Library/Application Support/stockwise/angel_config.json
```

Config file format:
```json
{
  "api_key": "YOUR_API_KEY",
  "client_id": "YOUR_ANGEL_CLIENT_ID",
  "password": "YOUR_LOGIN_PASSWORD",
  "totp_secret": "YOUR_TOTP_BASE32_SECRET"
}
```

### Trading

```bash
stockwise trade holdings              # View live holdings (auto-syncs locally)
stockwise trade buy RELIANCE 10       # Market buy 10 shares of Reliance
stockwise trade sell TCS 5            # Market sell 5 shares of TCS
stockwise trade limit INFY 20 1300    # Limit buy 20 Infosys at ₹1300
stockwise trade orders                # View today's order book
stockwise trade positions             # View open intraday positions
```

Your credentials are stored **locally only** and are never sent anywhere except directly to Angel One's API. TOTP is generated locally using the built-in HMAC-SHA1 implementation (no external dependencies).

---

## Broker Import

Import existing holdings from any broker without manual entry:

```bash
# Zerodha Kite: Portfolio → Holdings → Download CSV
stockwise import kite ~/Downloads/holdings.csv

# IndMoney: Portfolio → Export
stockwise import indmoney ~/Downloads/portfolio.csv

# Any broker with a CSV export (auto-detects columns)
stockwise import csv ~/Downloads/my_stocks.csv
```

The importer auto-detects column names (Symbol/Instrument, Quantity/Qty, Avg Cost/Average Price, etc.) so most CSV exports work out of the box.

---

## Indian Market Support

Indian market is the **default**. No flags needed:

```bash
stockwise quote RELIANCE TCS INFY       # Auto-resolves to .NS
stockwise longterm 5000                  # Best stocks for ₹5000/month SIP
stockwise bot 10000 2                    # Intraday: ₹10K capital, 2% target
stockwise tax in                         # Indian tax (STCG 20%, LTCG 12.5%)
```

- Prices display with `₹` symbol automatically
- Large numbers use **lakhs/crores** (e.g., `₹18.28L Cr`)
- BSE symbols work too: `stockwise quote RELIANCE.BO`
- Use `-m us` for US stocks: `stockwise -m us quote AAPL`

---

## Terminal Charts

StockWise renders rich visualizations directly in your terminal using Unicode:

- **Braille line charts** — high-resolution price charts
- **Dual-line overlays** — price vs SMA-20, portfolio value vs invested
- **Volume bars** — volume profile
- **Bar charts** — sector performance, portfolio allocation
- **RSI gauge** — visual overbought/oversold indicator
- **Fear & Greed gauge** — market sentiment
- **Equity curves** — backtest performance visualization

---

## Backtesting

Test 6 trading strategies on real historical data:

```bash
stockwise backtest RELIANCE -s rsi -p 2y     # RSI oversold/overbought
stockwise backtest TCS -s macd -p 5y         # MACD crossover
stockwise backtest INFY -s sma -p 2y         # SMA 20/50 crossover
stockwise backtest HDFCBANK -s bb -p 2y      # Bollinger Band bounce
stockwise backtest SBIN -s vwap -p 1y        # VWAP crossover
stockwise backtest ITC -s mr -p 2y           # Mean reversion (3% deviation)
```

Each backtest shows:
- Equity curve chart
- Total return vs buy-and-hold (alpha)
- Win rate, average win/loss
- Sharpe ratio, max drawdown
- Recent trade history

---

## Tax Calculator

Calculate estimated capital gains tax on your portfolio:

```bash
stockwise tax in    # India: STCG 20%, LTCG 12.5% (₹1.25L exempt)
stockwise tax us    # USA: STCG 37%, LTCG 20%
stockwise tax uk    # UK: STCG 20%, LTCG 20% (£3K exempt)
```

Automatically classifies each holding as short-term or long-term based on holding period, offsets losses against gains, and applies country-specific exemptions.

---

## Technical Indicators

- **Moving Averages**: SMA (10, 20, 50, 100, 200), EMA (12, 26)
- **Momentum**: RSI (14), MACD (12/26/9) with signal & histogram
- **Volatility**: Bollinger Bands (20,2), ATR (14), VWAP
- **Risk**: Annualized volatility, Sharpe/Sortino/Calmar ratios, VaR (95%/99%), max drawdown
- **Statistics**: Pearson correlation, linear regression, daily returns analysis

---

## Architecture

```
src/
  main.rs         CLI definition (clap) — 35+ subcommands
  api.rs          Yahoo Finance client with cookie/crumb auth
  angel.rs        Angel One SmartAPI — auth, TOTP, orders, holdings
  commands.rs     All command implementations
  technical.rs    RSI, MACD, SMA, EMA, Bollinger, ATR, VWAP, Sharpe, VaR, correlation
  backtest.rs     Backtesting engine — 6 strategies, trade simulation, equity curves
  intraday.rs     Intraday signal scoring and trade plan generation
  longterm.rs     Long-term multi-factor stock scoring
  insights.rs     AI insight generation and verdict scoring
  charts.rs       Braille line charts, bar charts, candlesticks, gauges
  display.rs      Currency-aware formatting, sparklines, colors
  market.rs       Market enum, symbol resolver, sector/index definitions
  portfolio.rs    Portfolio persistence (JSON)
  watchlist.rs    Watchlist persistence (JSON)
  alerts.rs       Price alert storage and checking
  wealth.rs       Daily portfolio snapshots for growth tracking
```

## Data Storage

All data is stored locally as JSON:

```
~/Library/Application Support/stockwise/    # macOS
~/.local/share/stockwise/                   # Linux
```

Files:
- `portfolio.json` — your holdings
- `watchlist.json` — watched symbols
- `alerts.json` — price alerts
- `wealth_history.json` — daily portfolio value snapshots
- `angel_config.json` — Angel One API credentials (local only)

---

## License

MIT
