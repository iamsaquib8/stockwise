# StockWise

Autonomous stock market CLI built in Rust. Supports **US** and **Indian** markets.

**57 commands** | **Daemon workers** | **Live trading** | **AI (Ollama)** | **Redis cache** | **89 tests**

## Install

```bash
cargo build --release
# or
cargo install --path .
```

## Quick Start

```bash
stockwise quote RELIANCE TCS INFY        # Indian stocks (default)
stockwise deep HDFCBANK                   # Everything about a stock
stockwise dashboard                       # Markets + portfolio + alerts
stockwise daemon intraday 25000           # Run the intraday bot all day
```

## What It Does

| Category | Commands |
|----------|----------|
| **Analysis** | `quote`, `analyze`, `technical`, `compare`, `history`, `deep`, `risk`, `correlate`, `timeframes`, `picks`, `earnings` |
| **Market** | `markets`, `movers`, `sectors`, `news`, `search`, `sentiment` |
| **Screening** | `screen`, `support`, `fibs`, `volume`, `gaps`, `stoploss`, `peers`, `dividends`, `options`, `insider`, `ipo`, `patterns` |
| **Bots** | `intraday` (9-strategy), `longterm` (6-pillar + Monte Carlo), `forecast`, `backtest`, `sip` |
| **Market** | `rotation`, `matrix`, `surprise`, `divcal`, `pcorr`, `returns`, `heatmap`, `sectorcmp`, `whatif` |
| **Daemon** | `daemon intraday` (continuous 9AM–3:30PM worker), `daemon longterm` (daily post-market analysis) |
| **Trading** | `trade buy/sell/limit/holdings/orders` (Angel One), `import kite/indmoney/csv` |
| **Portfolio** | `portfolio`, `watch`, `alert`, `export`, `wealth`, `rebalance`, `harvest`, `tax`, `dashboard` |
| **Simulator** | `sim start/status/settle/history` (paper trading + confidence rating) |
| **AI** | `report intraday/longterm/portfolio/<SYMBOL>` + auto-AI in 6 commands |

## Daemon Mode

Continuous background workers that monitor the market all day.

```bash
# Intraday worker — runs 9:00 AM to 3:30 PM IST
stockwise daemon intraday 25000

# What it does every day:
# 9:00   Pre-market scan (sectors, gaps)
# 9:15   Enter top 5 positions (9 strategies, Kelly sizing)
# 9:15–3:00  Monitor every 60s (trail stops, book T1 at 50%)
# 3:15   Square off everything
# 3:30   Settle → save to sim history → AI EOD report

# Long-term worker — run after market close
stockwise daemon longterm
# Scores 30 stocks → checks portfolio → alerts → tax harvest → AI memo
```

Risk controls: 1% per trade, 3% daily loss kill switch, max 5 positions.

## AI Integration

When [Ollama](https://ollama.com) is running locally, AI analysis is auto-added to `analyze`, `technical`, `compare`, `backtest`, `screen`, and daemon EOD reports. Default model: `qwen3:14b`.

```bash
ollama serve                              # Start Ollama
stockwise analyze RELIANCE                # Includes AI verdict
stockwise report HDFCBANK                 # Full AI stock report
```

## Live Trading (Angel One)

```bash
stockwise trade config                    # Setup credentials
stockwise trade buy RELIANCE 10           # Market buy
stockwise trade sell TCS 5                # Market sell
stockwise trade holdings                  # Sync live holdings
```

## Docs

Full documentation in [docs/](docs/):
- [Commands Reference](docs/commands.md) — all 50+ commands
- [Trading Bots](docs/bots.md) — intraday + longterm + simulator
- [Bot Design & Architecture](docs/bot-design.md) — daemon workers, state machine, risk controls
- [Angel One Setup](docs/angel-one.md) — live trading + broker import
- [AI & Ollama](docs/ai.md) — model setup, integration points
- [Architecture](docs/architecture.md) — source layout, data flow, testing

## License

MIT
