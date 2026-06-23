# StockWise

Autonomous stock market CLI built in Rust. Real-time quotes, deep analysis,
backtesting, trading bots, and live trading for **US** and **Indian** markets —
all from your terminal.

[![CI](https://github.com/iamsaquib8/stockwise/actions/workflows/ci.yml/badge.svg)](https://github.com/iamsaquib8/stockwise/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/stockwise.svg)](https://crates.io/crates/stockwise)
[![Downloads](https://img.shields.io/crates/d/stockwise.svg)](https://crates.io/crates/stockwise)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/rustc-1.85+-blue.svg)](#install)

**57 commands** · **daemon workers** · **live trading** · **local AI (Ollama)** · **Redis cache** · **213 tests**

> ⚠️ **Not financial advice.** StockWise is for education and information only.
> It can place **real-money orders** through your broker. You are responsible for
> every trade. Markets are risky — you can lose money. See
> [DISCLAIMER.md](DISCLAIMER.md) before using live trading.

![StockWise demo](assets/demo.gif)

<sub>Demo recorded with [VHS](https://github.com/charmbracelet/vhs) — regenerate with `vhs assets/demo.tape`.</sub>

## Install

### crates.io (recommended)

```bash
cargo install stockwise
```

### Prebuilt binaries

Grab a binary for macOS (Intel/Apple Silicon), Linux, or Windows from the
[latest release](https://github.com/iamsaquib8/stockwise/releases/latest).

### From source

```bash
git clone https://github.com/iamsaquib8/stockwise
cd stockwise
cargo install --path .       # needs Rust 1.85+
```

## Quick Start

```bash
stockwise quote RELIANCE TCS INFY        # Indian stocks (default market)
stockwise quote AAPL MSFT -m us          # US stocks
stockwise deep HDFCBANK                   # everything about a stock, one shot
stockwise markets                         # US + India indices at a glance
stockwise dashboard                       # markets + portfolio + alerts
```

```text
────────────────────────────────────────────────────────────
  AAPL — Apple Inc.
────────────────────────────────────────────────────────────
  $295.78  ▼ -1.23 (-0.41%)

  Day Range                    $295.18 — $301.64
  52-Week Range                $199.26 — $317.40
  Market Cap                   $4.34T
  P/E (TTM)                    35.81
  50-Day MA                    $289.47
  200-Day MA                   $268.49
```

## What It Does

| Category | Commands |
|----------|----------|
| **Analysis** | `quote`, `analyze`, `technical`, `compare`, `history`, `chart`, `deep`, `risk`, `correlate`, `timeframes`, `picks`, `earnings` |
| **Market** | `markets`, `movers`, `sectors`, `news`, `search`, `sentiment`, `rotation`, `matrix`, `surprise`, `heatmap`, `sectorcmp` |
| **Screening** | `screen`, `support`, `fibs`, `volume`, `gaps`, `stoploss`, `peers`, `dividends`, `options`, `insider`, `ipo`, `patterns` |
| **Bots** | `intraday` (9-strategy), `longterm` (6-pillar + Monte Carlo), `forecast`, `backtest`, `sip` |
| **Daemon** | `daemon intraday` (continuous 9 AM–3:30 PM worker), `daemon longterm` (daily post-market analysis) |
| **Trading** | `trade buy/sell/limit/holdings/orders` (Angel One), `import kite/indmoney/csv` |
| **Portfolio** | `portfolio`, `watch`, `alert`, `export`, `wealth`, `rebalance`, `harvest`, `tax`, `dashboard` |
| **Simulator** | `sim start/status/settle/history` (paper trading + confidence rating) |
| **AI** | `report intraday/longterm/portfolio/<SYMBOL>` + auto-AI in 6 commands |

Full reference: [docs/commands.md](docs/commands.md).

## Daemon Mode

Continuous background workers that monitor the market all day.

```bash
# Intraday worker — runs 9:00 AM to 3:30 PM IST
stockwise daemon intraday 25000

# 9:00   Pre-market scan (sectors, gaps)
# 9:15   Enter top 5 positions (9 strategies, Kelly sizing)
# 9:15–3:00  Monitor every 60s (trail stops, book T1 at 50%)
# 3:15   Square off everything
# 3:30   Settle → save to sim history → AI EOD report

# Long-term worker — run after market close
stockwise daemon longterm
```

Risk controls: 1% per trade, 3% daily-loss kill switch, max 5 positions. Read
[docs/bot-design.md](docs/bot-design.md) before running with real capital.

## AI Integration

When [Ollama](https://ollama.com) is running locally, AI analysis is auto-added
to `analyze`, `technical`, `compare`, `backtest`, `screen`, and daemon EOD
reports. Default model: `qwen3:14b`. Everything works without it.

```bash
ollama serve
stockwise analyze RELIANCE                # includes an AI verdict
stockwise report HDFCBANK                 # full AI stock report
```

## Live Trading (Angel One)

> Live trading uses **real money**. Start with the `sim` paper trader.

```bash
stockwise trade config                    # set up credentials (stored locally)
stockwise sim start 100000                # paper trade first
stockwise trade buy RELIANCE 10           # market buy
stockwise trade holdings                  # sync live holdings
```

## Docs

- [Commands Reference](docs/commands.md) — all 57 commands
- [Trading Bots](docs/bots.md) — intraday + longterm + simulator
- [Bot Design & Architecture](docs/bot-design.md) — daemon, state machine, risk
- [Angel One Setup](docs/angel-one.md) — live trading + broker import
- [AI & Ollama](docs/ai.md) — model setup, integration points
- [Architecture](docs/architecture.md) — source layout, data flow, testing

## Roadmap & Contributing

See the [ROADMAP](ROADMAP.md) for what's next (crates.io + binaries, more
brokers, a TUI dashboard, and more). Contributions are welcome — start with
[CONTRIBUTING.md](CONTRIBUTING.md) and the
[good first issues](https://github.com/iamsaquib8/stockwise/issues).

## License

[MIT](LICENSE) © iamsaquib8. Data via Yahoo Finance and your broker; StockWise is
not affiliated with any exchange, broker, or data provider. See
[DISCLAIMER.md](DISCLAIMER.md).
