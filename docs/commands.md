# Commands Reference

Default market is India. Use `-m us` for US stocks.

## Core Analysis

| Command | Alias | Description |
|---------|-------|-------------|
| `quote <SYMBOLS...>` | `q` | Live price, volume, P/E, market cap, MAs, 52-week range |
| `analyze <SYMBOL>` | `a` | Fundamental analysis: valuation, profitability, growth, analyst ratings, AI insights, verdict |
| `technical <SYMBOL>` | `t` | RSI, MACD, Bollinger Bands, ATR, VWAP, SMA/EMA, braille charts, AI interpretation |
| `compare <SYMBOLS...>` | `c` | Side-by-side comparison across 16 metrics with AI verdict |
| `history <SYMBOL> -p <PERIOD>` | | Price history with braille line chart, volume bars, SMA overlay. Periods: `1d`, `5d`, `1mo`, `3mo`, `6mo`, `1y`, `2y`, `5y`, `max` |
| `deep <SYMBOL>` | | Everything about a stock in one command — fundamentals, technicals, support/resistance, Fibonacci, risk, gaps, volume, insights, AI analysis |

## Market Intelligence

| Command | Description |
|---------|-------------|
| `markets` | US + India major indices (S&P 500, Dow, NASDAQ, NIFTY, SENSEX, Bank NIFTY) |
| `movers` | Top 10 gainers and losers from 30 popular large-cap stocks |
| `sectors` | Sector ETF/index performance with visual bars |
| `news <SYMBOL>` | Latest 10 headlines from Yahoo Finance |
| `search <QUERY>` | Find stock symbols by company name |
| `sentiment` | Fear & Greed gauge from VIX + market momentum + 1-month trend |

## Advanced Analysis

| Command | Alias | Description |
|---------|-------|-------------|
| `risk <SYMBOL>` | | Annualized volatility, Sharpe/Sortino/Calmar ratios, max drawdown, VaR (95%/99%), braille chart |
| `correlate <SYMBOLS...>` | `corr` | Pearson correlation matrix from 1Y daily returns |
| `screen <CATEGORY>` | | Screener: `undervalued` (P/E<15), `growth` (EPS fwd>TTM), `dividend` (yield>2%), `momentum` (top gainers), `bluechip` (cap>$100B) |
| `timeframes <SYMBOL>` | `mtf` | 1M/3M/6M/1Y/2Y/5Y returns, volatility, RSI, SMA/MACD signals at a glance |
| `picks` | | AI-scored top investment picks across 8 fundamental + technical factors |
| `earnings` | | Earnings growth overview for portfolio & watchlist stocks |

## Stock-Specific Tools

| Command | Description |
|---------|-------------|
| `support <SYMBOL>` | Pivot points (R1-R3, S1-S3) + Fibonacci retracement levels |
| `fibs <SYMBOL>` | Multi-timeframe Fibonacci (1M/3M/6M swings) with "you are here" marker |
| `volume <SYMBOL>` | On-Balance Volume chart, A/D line, volume ratio, 40-day volume bars |
| `gaps <SYMBOL>` | Detect unfilled/filled price gaps in last 3 months |
| `stoploss <SYMBOL> --entry <PRICE>` | ATR-based (1.5x/2x/3x), percentage (2-8%), Chandelier exit, R:R targets |
| `peers <SYMBOL>` | Auto-find same-sector peers, compare P/E, P/B, market cap |
| `dividends <SYMBOL>` | Yield analysis, income projection (₹1L/5L/10L invested), 10Y DRIP growth |
| `insider <SYMBOL>` | Analyst consensus + insider-related news |
| `options <SYMBOL>` | Expected moves (1/5/10/30 day), OTM strike suggestions |
| `ipo` | IPO calendar and latest IPO news |

## Wealth Generation

| Command | Description |
|---------|-------------|
| `backtest <SYM> -s <STRATEGY> -p <PERIOD>` | Backtest strategies: `rsi`, `macd`, `sma`, `bb`, `vwap`, `mr`. Shows equity curve, alpha, Sharpe, drawdown |
| `sip <SYM> <AMOUNT> -p <PERIOD>` | SIP/DCA simulator with value-vs-invested dual chart |
| `forecast <SYM> -d <DAYS>` | Linear trend projection with 95% confidence bands |
| `intraday <AMOUNT> <TARGET%>` | 9-strategy intraday bot with Kelly sizing, sector heat, dual targets |
| `longterm [AMOUNT]` | 6-pillar scoring, moat analysis, Monte Carlo (1000 sims), DRIP, SIP allocation |

## Portfolio & Tracking

| Command | Alias | Description |
|---------|-------|-------------|
| `portfolio add <SYM> <SHARES> <COST>` | `p` | Add a holding |
| `portfolio show` | `p` | Live P&L + allocation chart |
| `import <SOURCE> <FILE>` | | Import from `kite`, `indmoney`, or `csv` |
| `watch add <SYMBOLS...>` | `w` | Add to watchlist |
| `watch` | `w` | View watchlist with live prices |
| `alert add <SYM> above/below <PRICE>` | | Set price alert |
| `alert check` | | Check alerts against live prices |
| `export portfolio/watchlist` | | Export to CSV |
| `wealth` | | Snapshot portfolio value (run daily for growth chart) |
| `rebalance` | | Suggest trades for equal-weight allocation |
| `harvest` | | Find tax-loss harvesting opportunities |
| `tax <COUNTRY>` | | Tax calculator: `in` (India), `us` (USA), `uk` (UK) |
| `dashboard` | `d` | Combined: indices + portfolio + watchlist + triggered alerts |

## Trading & Simulation

| Command | Description |
|---------|-------------|
| `trade setup/config` | Configure Angel One SmartAPI credentials |
| `trade holdings` | Fetch live holdings (auto-syncs to StockWise) |
| `trade buy/sell <SYM> <QTY>` | Place market orders |
| `trade limit <SYM> <QTY> <PRICE>` | Place limit orders |
| `trade orders/positions` | View order book / open positions |
| `sim start [AMOUNT] [TARGET%]` | Start paper trading session |
| `sim status` | Live P&L on simulated trades |
| `sim settle` | End-of-day settlement |
| `sim history` | Cumulative performance + confidence rating |

## Daemon Workers

| Command | Description |
|---------|-------------|
| `daemon intraday [CAPITAL]` | Continuous intraday worker (9AM–3:30PM IST). Scans, enters, monitors every 60s, trails stops, books T1 at 50%, squares off at 3:15, settles EOD. Risk: 1%/trade, 3% daily kill switch |
| `daemon longterm` | Daily post-market analysis: score 30 stocks, check portfolio, alerts, tax harvest, AI memo |

## AI Reports

| Command | Description |
|---------|-------------|
| `report intraday` | AI morning trading brief |
| `report longterm` | AI investment memo |
| `report portfolio` | AI portfolio review |
| `report <SYMBOL>` | AI single-stock analysis |
