# StockWise

Autonomous stock market CLI built in Rust. Supports **US** and **Indian** markets.

**45+ commands** | **Live trading** | **AI-powered (local Ollama)** | **Paper trading simulator** | **61 tests**

## Install

```bash
cargo build --release
# or
cargo install --path .
```

## Quick Start

```bash
stockwise quote RELIANCE TCS INFY        # Indian stocks (default)
stockwise analyze HDFCBANK                # Deep fundamental analysis
stockwise technical TATAMOTORS            # Technical indicators + charts
stockwise -m us quote AAPL MSFT NVDA      # US stocks
stockwise dashboard                       # Everything at a glance
```

## What It Does

| Category | Commands |
|----------|----------|
| **Analysis** | `quote`, `analyze`, `technical`, `compare`, `history`, `risk`, `correlate`, `timeframes`, `picks`, `earnings` |
| **Market** | `markets`, `movers`, `sectors`, `news`, `search`, `sentiment` |
| **Screening** | `screen`, `support`, `fibs`, `volume`, `gaps`, `stoploss`, `peers`, `dividends`, `options` |
| **Bots** | `intraday` (9-strategy day trading), `longterm` (6-pillar + Monte Carlo), `forecast`, `backtest`, `sip` |
| **Trading** | `trade buy/sell/limit/holdings/orders` (Angel One), `import kite/indmoney/csv` |
| **Portfolio** | `portfolio`, `watch`, `alert`, `export`, `wealth`, `rebalance`, `harvest`, `tax`, `dashboard` |
| **Simulator** | `sim start/status/settle/history` (paper trading with performance tracking) |
| **AI Reports** | `report intraday/longterm/portfolio/<SYMBOL>` (local Ollama) |

## AI Integration

When [Ollama](https://ollama.com) is running, AI analysis is automatically added to `analyze`, `technical`, `compare`, `backtest`, and `screen` commands. Default model: `gemma3:12b`.

```bash
ollama serve                                    # Start Ollama
stockwise analyze RELIANCE                      # Includes AI analysis
STOCKWISE_MODEL=qwen3:14b stockwise report INFY # Use different model
```

## Live Trading (Angel One)

```bash
stockwise trade config                # Create credentials file
stockwise trade holdings              # Sync live holdings
stockwise trade buy RELIANCE 10       # Place market order
stockwise trade orders                # View order book
```

## Paper Trading Simulator

```bash
stockwise sim start 25000 2           # Morning: lock in bot trades (₹25K, 2% target)
stockwise sim status                  # During day: check live P&L
stockwise sim settle                  # EOD: record results
stockwise sim history                 # View performance + confidence rating
```

## Docs

Full documentation in [docs/](docs/):
- [Commands Reference](docs/commands.md)
- [Trading Bots](docs/bots.md)
- [Angel One Setup](docs/angel-one.md)
- [AI & Ollama](docs/ai.md)
- [Architecture](docs/architecture.md)

## License

MIT
