<p align="center">
  <img src="assets/banner.svg" alt="StockWise — stock market toolkit for your terminal" width="820">
</p>

<p align="center">
  <a href="https://github.com/iamsaquib8/stockwise/actions/workflows/ci.yml"><img src="https://github.com/iamsaquib8/stockwise/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/stockwise"><img src="https://img.shields.io/crates/v/stockwise.svg" alt="crates.io"></a>
  <a href="https://crates.io/crates/stockwise"><img src="https://img.shields.io/crates/d/stockwise.svg" alt="Downloads"></a>
  <a href="https://github.com/iamsaquib8/stockwise/releases/latest"><img src="https://img.shields.io/github/v/release/iamsaquib8/stockwise?label=binaries" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="#install"><img src="https://img.shields.io/badge/rustc-1.97+-blue.svg" alt="MSRV 1.97"></a>
</p>

<p align="center">
  <a href="https://iamsaquib8.github.io/stockwise/"><b>Docs</b></a> ·
  <a href="docs/cheatsheet.md">Cheatsheet</a> ·
  <a href="#install">Install</a> ·
  <a href="#tour">Tour</a> ·
  <a href="docs/commands.md">Commands</a> ·
  <a href="docs/comparison.md">Compare</a> ·
  <a href="#configuration">Configuration</a> ·
  <a href="CONTRIBUTING.md">Contributing</a> ·
  <a href="ROADMAP.md">Roadmap</a>
</p>

**StockWise is a stock market toolkit for your terminal, built for Indian
(NSE/BSE) and US stocks.** Quotes, charts, screeners, backtests, SIP math, paper
trading, and portfolio tracking in one fast Rust binary. No API keys, no
account, no browser tabs.

<p align="center">
  <img src="assets/history.svg" alt="stockwise history AAPL -m us -p 6mo: a braille price chart with volume bars in the terminal" width="760">
</p>

```bash
stockwise quote RELIANCE TCS            # Indian stocks are the default
stockwise quote AAPL NVDA -m us         # US stocks
stockwise deep HDFCBANK                 # everything about one stock
stockwise backtest INFY -s macd         # test a strategy, after STT and brokerage
stockwise sip NIFTYBEES 5000 -p 5y      # what a ₹5,000/month SIP became (XIRR)
```

> **Not financial advice.** StockWise is for education and information. See
> [DISCLAIMER.md](DISCLAIMER.md).

## Why StockWise

- **India-first, US too.** `TCS` means `TCS.NS`; rupee formatting in lakhs and
  crores; NIFTY/SENSEX sectors; mutual funds from AMFI; NSE option chains;
  Indian tax years with LTCG/STCG set-off; imports from Kite, Groww, INDmoney,
  Upstox and CAS statements; gold in ₹/10 g, Sovereign Gold Bonds and G-sec
  yields. `-m us`, `-m crypto`, `-m fx`, `-m commodity` or a global exchange
  (`-m uk`, `de`, `jp`, `hk`, `sg`, `ca`, `au`) for other markets.
- **Zero setup.** Public market data, no API key or sign-up. One binary for
  macOS, Linux and Windows. `stockwise doctor` checks everything.
- **75+ commands, one muscle memory.** Fundamentals, technicals, risk, screeners,
  sector rotation, mutual funds, options greeks, backtests, SIP/what-if
  calculators, goal planning, alerts, tax estimates and ITR-ready capital-gains
  exports — in English or Hindi (`--lang hi`).
- **Backtests that count costs.** STT, brokerage, stamp duty and slippage;
  next-day fills; portfolios and walk-forward; strategies as small TOML files.
- **Scriptable.** `--json` on 21 data commands and `--csv` on their tables, a
  config file for your defaults, real exit codes, shell completions, and a
  read-only JSON API with Prometheus metrics (`stockwise serve`).
- **Live dashboards.** `stockwise tui` keeps indices, watchlist, portfolio P&L,
  alerts and a chart on one refreshing screen; `stockwise serve --open` shows
  the same in your browser.
- **Practice before money.** A paper-trading simulator, an intraday paper
  daemon with risk controls and notifications, and historical replays — trade
  the 2020 crash with the future hidden — scored on risk, not raw return. Optional live orders via Angel
  One (Zerodha, Upstox, Dhan and Fyers experimental), always behind a preview
  and a typed confirmation.
- **Private by design.** Your portfolio stays in local files; broker secrets in
  your OS keyring. Optional AI runs on your own machine through
  [Ollama](https://ollama.com), with cloud models strictly opt-in.

## Install

**macOS, Linux and Termux (Android) — prebuilt binary**

```bash
curl -fsSL https://raw.githubusercontent.com/iamsaquib8/stockwise/master/install.sh | sh
```

Installs to `~/.local/bin` (`$PREFIX/bin` on Termux, releases after 0.5.0;
override with `STOCKWISE_INSTALL=/usr/local/bin`) and verifies the release
checksum. Every
[release](https://github.com/iamsaquib8/stockwise/releases/latest) also has
`.tar.gz` / `.zip` archives with the binary and man pages.

**With Python tools** — any OS, no Rust needed (releases after 0.5.0)

```bash
pipx install stockwise                  # or: uv tool install stockwise
uvx stockwise quote RELIANCE TCS        # run it once without installing
```

**Package managers**

```bash
brew install iamsaquib8/tap/stockwise            # macOS/Linux, Homebrew (after the tap is published)
yay -S stockwise-bin                             # Arch Linux, AUR (after the package is published)
nix profile install github:iamsaquib8/stockwise  # Nix flake (try it: nix run github:iamsaquib8/stockwise)
```

**Debian, Ubuntu, Fedora, RHEL** — `.deb` / `.rpm` for x86_64 and arm64 (releases after 0.5.0; glibc 2.34+: Debian 12, Ubuntu 22.04, RHEL 9 and newer)

```bash
curl -fsSLO https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise_amd64.deb && sudo apt install ./stockwise_amd64.deb
sudo dnf install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise-x86_64.rpm
```

On ARM use `stockwise_arm64.deb` / `stockwise-aarch64.rpm`.

**Windows**

```powershell
scoop install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise.json
winget install iamsaquib8.StockWise        # after the manifest is accepted into winget-pkgs
```

**Docker** (linux/amd64 and linux/arm64)

```bash
docker run --rm -it -v stockwise-data:/data ghcr.io/iamsaquib8/stockwise quote RELIANCE TCS
```

Portfolio, watchlist and alerts persist in the `stockwise-data` volume. The
same image is mirrored to Docker Hub as `iamsaquib8/stockwise` (once the mirror
is set up).

**With Rust**

```bash
cargo binstall stockwise    # prebuilt binary via cargo-binstall
cargo install stockwise     # or build from source (Rust 1.97+)
```

On the way: `pkg install stockwise` in Termux and `nix profile install
nixpkgs#stockwise`, once the [prepared packages](packaging/README.md) are
submitted and accepted. Every option, with what it includes and how it
updates: [Install](docs/install.md).

Then:

```bash
stockwise doctor                                          # check your setup
stockwise completions zsh > ~/.zfunc/_stockwise           # optional: tab completion
stockwise man --dir ~/.local/share/man/man1               # optional: man stockwise
```

Homebrew, AUR, Nix and the `.deb`/`.rpm` packages install the man pages and
completions for you.

## Tour

<p align="center">
  <img src="assets/markets.svg" alt="stockwise markets: Indian and US indices with daily change" width="49%">
  <img src="assets/backtest.svg" alt="stockwise backtest TCS -s macd: equity curve after costs, net vs gross return, trade statistics" width="49%">
</p>

| | Commands |
|---|---|
| **Start here** | `quote`, `deep`, `markets`, `dashboard`, `search`, `doctor` |
| **Analysis** | `analyze`, `technical`, `history`, `chart`, `compare`, `risk`, `correlate`, `timeframes`, `returns`, `patterns`, `support`, `fibs`, `volume`, `gaps`, `stoploss`, `forecast` |
| **Market** | `movers`, `sectors`, `heatmap`, `sentiment`, `news`, `rotation`, `sectorcmp`, `peers`, `insider`, `ipo`, `earnings`, `surprise`, `matrix`, `picks`, `screen`, `dividends` |
| **Funds & F&O** | `mf` (search, nav, history, sip, screen, overlap — with AMFI expense ratios), `options` (chain, pcr, maxpain, payoff, moves; NSE, BSE and US) |
| **Commodities & bonds** | `commodity` (spot, futures, etfs, sgb), `bonds` (yields, list, calc) |
| **Strategies & bots** | `backtest`, `sip`, `whatif`, `intraday`, `longterm`, `daemon`, `sim`, `report` |
| **Portfolio** | `portfolio`, `watch`, `alert`, `calendar`, `import`, `export`, `wealth`, `rebalance`, `harvest`, `tax`, `divcal`, `pcorr` |
| **Planning** | `goal` (sip, retire, education) |
| **Interfaces** | `tui`, `serve` |
| **Live trading** | `trade` (Angel One; Zerodha, Upstox, Dhan, Fyers experimental) |

`stockwise --help` shows the same groups; [docs/commands.md](docs/commands.md)
has every option.

### Track a portfolio

```bash
stockwise import kite ~/Downloads/holdings.csv   # also groww, upstox, cas; or: portfolio add TCS 10 3500 --date 2023-04-12
stockwise portfolio                              # live P&L, XIRR + allocation
stockwise portfolio sell TCS 5 3900              # FIFO sale, realized P&L kept for tax
stockwise portfolio actions --apply              # splits, bonuses and dividends since you bought
stockwise tax in                                 # LTCG/STCG per lot, set-off, realized vs unrealized
stockwise tax export in --fy 2025-26 > cg.csv    # ITR Schedule CG / 112A CSV of your sales
stockwise portfolio benchmark NIFTY -p 1y        # time-weighted return vs the index, alpha/beta, attribution
stockwise --portfolio us portfolio add AAPL 3 190 -m us   # a second, named portfolio
stockwise dashboard                              # indices + portfolio + watchlist + alerts
```

Every purchase is a lot with its own date, so holding periods, FIFO sales and
XIRR are exact. Holdings in INR and USD are totalled in one currency
(`portfolio --base usd`) at the live `USDINR=X` rate.

### Plan a goal

```bash
stockwise goal sip --target 1cr --years 15 --step-up 10       # SIP for ₹1 crore, rising 10% a year
stockwise goal retire --monthly-expense 80k --years-to-retire 20 --years-in-retirement 30
stockwise goal education --cost 25L --years 10                # cost after 8% education inflation
```

### Mutual funds and options

```bash
stockwise mf search parag flexi direct     # AMFI scheme codes
stockwise mf sip 122639 5000 -p 5y         # monthly SIP with XIRR
stockwise mf screen flexi cap              # rank a category by 3Y CAGR, with TERs
stockwise mf screen elss -s ter --max-ter 1  # cheapest ELSS funds by expense ratio
stockwise options chain NIFTY              # NSE chain with IV and greeks (SENSEX → BSE)
stockwise options maxpain BANKNIFTY        # or: options pcr, options chain AAPL -m us
stockwise options payoff NIFTY -s iron-condor -w 200   # P/L chart, breakevens, POP
```

NAVs and expense ratios come from AMFI, option chains from NSE or BSE (India) or Yahoo (US). See
[docs/commands.md](docs/commands.md#funds--fo) for what has data and what doesn't.

### Gold, SGBs and G-secs

```bash
stockwise commodity                          # gold ₹/10 g, silver ₹/kg, and USD/oz
stockwise commodity sgb                      # Sovereign Gold Bonds vs gold spot
stockwise history gold -m commodity -p 1y    # COMEX gold (GC=F); also chart, technical, alerts
stockwise bonds                              # T-bill and G-sec yields, India 10Y
stockwise bonds calc 718GS2033 --price 101.25   # yield to maturity, accrued interest, duration
```

Spot is COMEX futures at the USD/INR rate, before import duty and GST; MCX has
no free feed. Yields come from CCIL (RBI as fallback). Nothing is invented when
a source is down — see [what has data](docs/commands.md#commodities--bonds).

### Backtest a strategy

```bash
stockwise backtest INFY -s macd                    # net of Indian delivery costs, next-open fills
stockwise backtest portfolio TCS INFY HDFCBANK -s sma --rebalance monthly
stockwise backtest walkforward RELIANCE -s rsi --train 1y --test 3mo   # is it overfit?
stockwise backtest TCS -s file:my_rules.toml       # your own TOML rules
```

Strategies are TOML files of entry and exit rules over RSI, MACD, moving
averages, Bollinger Bands, ATR and VWAP — see
[docs/backtesting.md](docs/backtesting.md). For logic rules can't express,
an experimental build (`--features wasm-plugins`) runs sandboxed WebAssembly
strategies: `-s wasm:strategy.wasm`
([example](examples/wasm-strategy-rsi/)).

### Watch it live

<p align="center">
  <img src="assets/tui.svg" alt="stockwise tui: indices, watchlist, a price chart, portfolio P&L and alerts on one screen" width="760">
</p>

```bash
stockwise tui                # ↑↓ select · 1-6 chart period · / add to watchlist · ? help · q quit
```

Read-only: the TUI never places orders. See [docs/tui.md](docs/tui.md).

Prefer a browser? `stockwise serve --open` serves the same view as a web page —
indices, a chart with period buttons, watchlist, portfolio with allocation and
XIRR, alerts and search — refreshing every 30 seconds, on a phone too.

<p align="center">
  <img src="assets/web-dashboard.png" alt="StockWise web dashboard in a browser: indices, a price chart, watchlist, portfolio and alerts" width="760">
</p>

```bash
stockwise serve --open       # http://127.0.0.1:8787/ — read-only, localhost only
```

See [docs/server.md](docs/server.md#web-dashboard).

### Script it

`--json` works on the data commands (`portfolio`, `watch`, `alert`, `technical`,
`risk`, `backtest`, `screen`, `movers`, `compare`, `tax`, ...) and `--csv` on
their tables. Only the data goes to stdout; errors go to stderr with a non-zero
exit code.

```bash
stockwise portfolio --json | jq '.totals.pnl_percent'
stockwise portfolio --csv > portfolio.csv                # straight into a spreadsheet
stockwise history AAPL -m us -p 6mo --csv > aapl.csv     # every OHLCV bar
stockwise alert check --json | jq -r '.[] | select(.status == "triggered") | .symbol'
stockwise technical TCS --json | jq '{rsi_14, bias}'
stockwise backtest INFY -s macd --json | jq '.total_return_percent'
stockwise quote ZZZZ || echo "lookup failed"             # non-zero exit on errors
stockwise serve                                          # the same JSON over HTTP + /metrics
curl -s localhost:8787/api/portfolio | jq '.totals'
```

Every JSON shape and CSV column is listed in
[docs/commands.md](docs/commands.md#scripting-with---json-and---csv).

`serve` has no trading endpoints and refuses non-loopback addresses unless you
pass `--allow-remote` with a `STOCKWISE_API_TOKEN`. See
[docs/server.md](docs/server.md).

### Paper trade, then decide

```bash
stockwise sim start 100000          # paper session with the bot's picks
stockwise sim status                # live P&L
stockwise daemon intraday 25000     # NSE-hours paper loop: 1% risk/trade, 3% daily stop
stockwise sim settle                # after the close: replay the day's 5-minute bars
stockwise sim history               # track record + confidence rating
```

The daemon never sends broker orders. It runs on the exchange clock (IST,
NSE holidays included), survives network blips, resumes after a restart, and
squares off on Ctrl+C. Read [docs/bot-design.md](docs/bot-design.md) for how
it decides.

### Practise on past markets

```bash
stockwise sim replay start covid-crash-2020      # Feb 2020, ₹1,00,000 of paper money, future hidden
stockwise sim challenge start crash-survivor     # optional goal: lose less than half of what NIFTY loses
stockwise sim replay buy TCS 10 --stop 1900      # fills at the next day's open, with Indian charges
stockwise sim replay next 5                      # step through trading days
stockwise sim replay end                         # 0–100 score, badges, and what really happened
stockwise sim card --svg card.svg                # share the result
```

<p align="center"><img src="assets/sim-card.svg" alt="A StockWise replay result card: the COVID crash, score 74, grade B" width="560"></p>

Six built-in scenarios (2008, the dot-com bust, the COVID crash, the 2021 bull
run, the Adani report, the 2022 rate hikes) or your own dates and stocks. The
score rewards risk-adjusted return, small drawdowns, stop-losses, sensible
position sizes and diversification — never raw return alone. Paper money
only; see [docs/simulator.md](docs/simulator.md).

### Alerts and notifications

```bash
stockwise alert add TCS above 4200
stockwise alert add INFY rsi-below 30         # also: SMA/EMA crosses, breakouts, volume spikes
stockwise alert add TCS results-in 7          # results due within a week
stockwise calendar                  # results, ex-dividend dates and splits for your holdings
export STOCKWISE_TELEGRAM_TOKEN=... STOCKWISE_TELEGRAM_CHAT_ID=...
stockwise notify test               # Telegram, Slack, Discord, email or desktop
stockwise alert watch               # checks during market hours, notifies once per trigger
```

See [notifications](docs/bots.md#notifications) for every channel.

### Live orders (optional)

```bash
stockwise trade setup                          # how to connect Angel One
stockwise trade buy RELIANCE 10 --dry-run      # resolve + preview, send nothing
stockwise trade buy RELIANCE 10                # preview, then type "yes" to send
stockwise trade sell RELIANCE 10 --trigger 1250            # stop-loss (SL-M)
stockwise trade holdings --broker zerodha      # also: upstox, dhan, fyers (experimental)
stockwise trade sync --broker zerodha --apply  # today's fills → dated lots and FIFO sales
stockwise trade selftest --broker zerodha      # read-only check; paste the report in an issue
```

Orders resolve the exact NSE instrument, show the estimated value, respect an
optional `max_order_value` cap, and refuse to run unattended without `--yes`.
Broker secrets live in the OS keyring when one is available. Zerodha Kite,
Upstox, Dhan and Fyers are **experimental — untested against a live account**:
read-only until you opt in. See [docs/brokers.md](docs/brokers.md).

### Local AI (optional)

With [Ollama](https://ollama.com) running, `stockwise report TCS` writes
educational AI notes on a stock (signals, risks, what to watch — never buy/sell
calls), and `--ai` adds commentary to `analyze`, `technical`, `compare`,
`screen`, `backtest` and `deep`. It's opt-in because local models take 30–60s.
Prefer a hosted model? Set `STOCKWISE_AI_PROVIDER` to `openai`, `anthropic` or
`gemini` with your key — still only for `report`, `ask` and `--ai`, with a
notice whenever data is sent. See [docs/ai.md](docs/ai.md).

```bash
stockwise ask "how is my portfolio doing this month?"     # the model calls StockWise's tools
stockwise ask "compare TCS and INFY on RSI and risk" -v   # -v shows each tool call
```

### Global exchanges, crypto, forex and Hindi

```bash
stockwise quote VOD BARC -m uk                       # VOD.L, BARC.L — London, in pence
stockwise quote 7203 -m jp                           # Toyota, 7203.T (also de, hk, sg, ca, au)
stockwise markets --global                           # FTSE, DAX, Nikkei, Hang Seng, ... open or closed now
stockwise quote BTC ETH -m crypto                    # BTC-USD, ETH-USD — trades 24/7
stockwise quote BTC -m crypto --quote-currency inr   # BTC-INR
stockwise quote USDINR EURUSD -m fx                  # USDINR=X, EURUSD=X, 4 decimals
stockwise markets --crypto --fx                      # indices + top coins + major pairs
stockwise dashboard --lang hi                        # हिन्दी में (also STOCKWISE_LANG=hi)
```

## Use with AI assistants

`stockwise mcp` is an [MCP](https://modelcontextprotocol.io) server: Claude
Desktop, Claude Code, Cursor and other assistants can look up quotes,
technicals, risk, mutual funds, option chains, screens and backtests, and read
your portfolio, lots, tax estimate, calendar, watchlist and alerts — 24
read-only tools, three resources and ready-made prompts ("review my
portfolio", "tax-harvest check before 31 March", "explain this option chain").
It never places orders.

```bash
claude mcp add stockwise -- stockwise mcp        # Claude Code
```

Claude Desktop (Settings → Developer → Edit Config) and Cursor
(`~/.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "stockwise": { "command": "stockwise", "args": ["mcp"] }
  }
}
```

Without installing anything but [uv](https://docs.astral.sh/uv/), once the PyPI
wheels are out: `"command": "uvx", "args": ["stockwise", "mcp"]`. Over HTTP:
`stockwise mcp --http 127.0.0.1:8788`. Agents can also read the
[Agent Skill](skills/stockwise/SKILL.md) and the docs'
[llms.txt](https://iamsaquib8.github.io/stockwise/llms.txt). Details, the tool
list and the safety model: [docs/ai.md](docs/ai.md#use-stockwise-from-ai-assistants-mcp).

- MCP registry: mcp-name: io.github.iamsaquib8/stockwise

## Configuration

Everything works with no config. To keep your own defaults, create a config
file:

```bash
stockwise config init     # writes a commented config.toml
stockwise config show     # effective settings and where each comes from
```

```toml
market = "us"
model = "llama3.1:8b"

[periods]                 # default -p per command
technical = "6mo"
backtest = "5y"
```

Each setting can also come from an environment variable or a flag.
Precedence: **flag > environment variable > config file > built-in default.**

| Config key | Env var | Flag | Purpose |
|------------|---------|------|---------|
| `market` | `STOCKWISE_MARKET` | `-m` | Default market: `in` (default), `us`, `crypto`, `fx`, `commodity`, or a global exchange: `uk`, `de`, `jp`, `hk`, `sg`, `ca`, `au` |
| `quote_currency` | `STOCKWISE_QUOTE_CURRENCY` | `--quote-currency` | Currency for bare crypto tickers with `-m crypto` (default `usd`; `inr` gives `BTC-INR`) |
| `lang` | `STOCKWISE_LANG` | `--lang` | Output language: `en` or `hi` (Hindi). Default: your locale (`LANG=hi_IN…` picks Hindi) |
| `home` | `STOCKWISE_HOME` | | Where portfolio, watchlist, alerts and history are stored |
| `portfolio` | `STOCKWISE_PORTFOLIO` | `--portfolio` | Named portfolio to use |
| `data_provider` | `STOCKWISE_DATA_PROVIDER` | | Opt-in data provider: `alphavantage`, `massive` (Polygon) or `twelvedata` with its API key in the environment, or the exchanges' own websites — `nse`, `bse` or `exchange` (no key) — see [docs/data-providers.md](docs/data-providers.md) |
| `ai` | `STOCKWISE_AI` | `--ai` / `--no-ai` | Add AI sections by default |
| `ai_provider` | `STOCKWISE_AI_PROVIDER` | | `ollama` (default, local), or opt-in cloud: `openai`, `anthropic`, `gemini` with `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `GEMINI_API_KEY` in the environment — see [docs/ai.md](docs/ai.md) |
| `model` | `STOCKWISE_MODEL` | | AI model (Ollama default `qwen3:14b`) |
| `ollama_host` | `OLLAMA_HOST` | | Ollama server (default `http://localhost:11434`) |
| `redis_url` | `STOCKWISE_REDIS_URL` | | Optional shared cache (`redis://…`), or `off` |
| `[periods] <command>` | | `-p` | Default period for `technical`, `history`, `risk`, `backtest`, `sip`, ... |
| `[notify] telegram_token`, ... | `STOCKWISE_TELEGRAM_TOKEN`, `STOCKWISE_SLACK_WEBHOOK`, … | | Notification channels — see [docs/bots.md](docs/bots.md#notifications) |
| | `STOCKWISE_BROKER` | `trade --broker` | Default broker for `trade`: `angel` (default), `zerodha`, `upstox`, `dhan`, `fyers` |
| | `STOCKWISE_KEYRING` | | `off` keeps broker secrets in the owner-only file instead of the OS keyring |
| | `STOCKWISE_BIND` / `STOCKWISE_API_TOKEN` | `serve --bind` | `serve` address and bearer token |
| | `STOCKWISE_CONFIG` | | Use another config file |
| | `NO_COLOR` | | Disable colours (also automatic when piping) |

The file lives in your platform's config directory (`stockwise config path`
prints it); see [docs/commands.md](docs/commands.md#configuration-file) for
details. API keys and the server token are read from the environment only.

The same settings work as flags: `-m crypto`, `--quote-currency inr`,
`--lang hi`, `--ai`. Run `stockwise doctor` to see what is active.

## Docs

The full documentation is at **[iamsaquib8.github.io/stockwise](https://iamsaquib8.github.io/stockwise/)**.

- [Cheatsheet](docs/cheatsheet.md) — the commands people use most, on one page
- [How StockWise compares](docs/comparison.md) with other tools
- [Commands reference](docs/commands.md)
- [Markets: global exchanges, commodities, bonds](docs/markets.md)
- [Terminal dashboard](docs/tui.md) and [web dashboard, HTTP API & metrics](docs/server.md)
- [Trading bots](docs/bots.md) and [bot design](docs/bot-design.md)
- [Practice mode: replays, challenges and scores](docs/simulator.md)
- [Brokers: live trading, keyring, imports](docs/brokers.md)
- [AI: `ask`, the MCP server, Ollama and cloud providers](docs/ai.md)
- [Data providers](docs/data-providers.md)
- [Architecture](docs/architecture.md)

## Roadmap & contributing

Next up: `pipx`/`uvx` installs, published Homebrew/winget/Scoop/AUR packages, and tax-season tools — see the
[ROADMAP](ROADMAP.md). Ideas and bug reports are
welcome in [Discussions](https://github.com/iamsaquib8/stockwise/discussions) and
[Issues](https://github.com/iamsaquib8/stockwise/issues); start with
[CONTRIBUTING.md](CONTRIBUTING.md).

If StockWise is useful to you, a ⭐ helps other people find it.

## License

[MIT](LICENSE) © iamsaquib8. Market data comes from Yahoo Finance and your
broker; StockWise is not affiliated with any exchange, broker, or data provider.
See [DISCLAIMER.md](DISCLAIMER.md).
