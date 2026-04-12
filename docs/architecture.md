# Architecture

## Source Layout

```
src/
  main.rs          CLI definition (clap) — 45+ subcommands
  commands.rs      All command implementations (~3500 lines)
  api.rs           Yahoo Finance client (cookie/crumb auth, quotes, charts, search)
  angel.rs         Angel One SmartAPI (login, TOTP, orders, holdings, positions)
  ai.rs            Ollama integration (prompts, stock analysis, reports)
  technical.rs     25+ indicators (SMA, EMA, RSI, MACD, BB, ATR, VWAP, OBV, Fibonacci, pivot points, VaR, Sharpe, correlation)
  backtest.rs      Backtesting engine (6 strategies, trade execution, equity curves)
  intraday.rs      Intraday bot (9 strategies, regime detection, Kelly sizing, sector heat)
  longterm.rs      Long-term bot (6-pillar scoring, moat analysis, Monte Carlo)
  simulator.rs     Paper trading (session tracking, daily P&L, cumulative stats)
  insights.rs      Rule-based investment insight generation
  charts.rs        Braille line charts, dual overlays, volume bars, bar charts, gauges
  display.rs       Currency-aware formatting (₹/$/£), sparklines, colors
  market.rs        Market enum, symbol resolver, sector/index definitions, stock lists
  portfolio.rs     Portfolio persistence (JSON)
  watchlist.rs     Watchlist persistence (JSON)
  alerts.rs        Price alert storage and checking
  wealth.rs        Daily portfolio snapshots
```

## Data Flow

```
User Input → clap CLI → commands.rs → api.rs → Yahoo Finance
                                   → angel.rs → Angel One
                                   → ai.rs → Local Ollama
                                   → technical.rs (indicators)
                                   → intraday.rs / longterm.rs (bots)
                                   → charts.rs (visualization)
                                   → display.rs (formatting)
                                   → portfolio.rs / watchlist.rs (persistence)
```

## Data Storage

Location: `~/Library/Application Support/stockwise/` (macOS) or `~/.local/share/stockwise/` (Linux)

| File | Purpose |
|------|---------|
| `portfolio.json` | Holdings (symbol, shares, avg cost, date) |
| `watchlist.json` | Watched symbols |
| `alerts.json` | Price alerts (symbol, condition, target) |
| `wealth_history.json` | Daily portfolio value snapshots |
| `sim_history.json` | Paper trading sessions and results |
| `angel_config.json` | Angel One API credentials |

## Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing (derive macros) |
| `reqwest` | HTTP client (Yahoo Finance, Angel One, Ollama) |
| `tokio` | Async runtime |
| `serde` / `serde_json` | JSON serialization |
| `colored` | Terminal colors |
| `chrono` | Date handling |
| `dirs` | Platform-specific data directories |
| `anyhow` | Error handling |

## Testing

61 unit tests across 6 modules:

```bash
cargo test              # Run all tests
cargo test technical    # Run only technical indicator tests
cargo test longterm     # Run only longterm bot tests
```

Tested modules: `technical` (18), `backtest` (11), `display` (10), `charts` (9), `longterm` (9), `market` (4).
