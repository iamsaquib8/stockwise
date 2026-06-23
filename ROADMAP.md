# Roadmap

Where StockWise is headed. Dates are intentionally omitted — this is a
direction, not a promise. Have an opinion? Open a
[Discussion](https://github.com/iamsaquib8/stockwise/discussions) or upvote an
[issue](https://github.com/iamsaquib8/stockwise/issues) — the roadmap follows
demand.

Legend: ✅ shipped · 🛠️ in progress · 📋 planned · 💡 exploring

## ✅ Recently shipped (0.2)

- 57 commands across analysis, screening, bots, trading, and portfolio
- Daemon workers — continuous intraday (9-strategy) + daily long-term
- Paper-trading simulator with confidence rating
- Live trading via Angel One; broker import (Kite, INDmoney, CSV)
- Local AI via Ollama, auto-injected into 6 commands
- Redis caching with in-memory fallback
- Multi-timeframe + braille terminal charts
- 205 tests, edition 2024

## 🛠️ Now — distribution & scripting (0.2.x)

- 🛠️ Publish to crates.io — `cargo install stockwise`
- 🛠️ Prebuilt binaries for macOS (x86_64/arm64), Linux, Windows on every release
- 📋 Homebrew tap — `brew install iamsaquib8/tap/stockwise`
- 📋 Global `--json` flag so every command pipes into `jq`, dashboards, and scripts
- 📋 Shell completions (bash/zsh/fish) + `stockwise completions` command

## 📋 Next — brokers, config & alerts (0.3)

- 📋 Native Zerodha Kite and Upstox brokers (beyond CSV import)
- 📋 Config profiles (`~/.config/stockwise/config.toml`) + named portfolios
- 📋 Secure credential storage via the OS keyring (no plaintext broker secrets)
- 📋 Daemon notifications — Telegram, Slack, and email on entries/exits/alerts
- 📋 Optional cloud AI providers (OpenAI, Anthropic, Gemini) alongside Ollama

## 💡 Later — depth (0.4+)

- 💡 Interactive TUI dashboard (ratatui) — live quotes, positions, P&L
- 💡 Portfolio-level and walk-forward backtesting
- 💡 Pluggable strategy system for `backtest` and the intraday bot
- 💡 Crypto and forex symbols
- 💡 Mutual-fund / ETF screening; options-chain greeks
- 💡 Web/REST mode for remote dashboards

## Non-goals

- Becoming a brokerage or holding custody of funds
- Promising profits or providing personalized financial advice (see
  [DISCLAIMER.md](DISCLAIMER.md))
- High-frequency / co-located trading
