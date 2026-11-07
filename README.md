<p align="center">
  <img src="assets/banner.svg" alt="StockWise — stock market toolkit for your terminal" width="820">
</p>

<p align="center">
  <a href="https://github.com/iamsaquib8/stockwise/actions/workflows/ci.yml"><img src="https://github.com/iamsaquib8/stockwise/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/stockwise"><img src="https://img.shields.io/crates/v/stockwise.svg" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://iamsaquib8.github.io/stockwise/"><img src="https://img.shields.io/badge/docs-site-informational.svg" alt="Docs"></a>
</p>

**StockWise is a command-line tool for investors in Indian (NSE/BSE) and US
markets.** It tracks your portfolio with exact lots and XIRR, works out Indian
capital-gains tax, looks up stocks and mutual funds, and lets an AI assistant
read your portfolio — with your data kept in local files. One binary, no account,
no API key.

<p align="center">
  <img src="assets/history.svg" alt="stockwise history AAPL -m us -p 6mo: a braille price chart with volume bars in the terminal" width="760">
</p>

> **Not financial advice.** StockWise is for education and information. See
> [DISCLAIMER.md](DISCLAIMER.md).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/iamsaquib8/stockwise/master/install.sh | sh   # macOS, Linux, Termux
cargo binstall stockwise                                                                    # or: cargo install --locked stockwise
```

Also `.deb`/`.rpm`, Scoop, Nix and Docker — see [Install](docs/install.md) for
every option. PyPI, Homebrew, winget and the AUR are not published yet. Building
from source needs Rust 1.97 or newer. Then run `stockwise doctor` to check your
setup.

## What it does

### One portfolio, true returns

```bash
stockwise import kite ~/Downloads/holdings.xlsx   # CSV or .xlsx from Kite, Groww, Upstox, INDmoney; CAS too
stockwise portfolio                               # live P&L, XIRR and allocation
stockwise portfolio sell TCS 5 3900               # FIFO sale; realised P&L kept for tax
stockwise portfolio benchmark NIFTY -p 1y         # return vs the index
stockwise portfolio xray                          # real exposure across stocks, funds and ETFs
stockwise portfolio target set TCS 30             # target weights; then rebalance --inflow 50000
```

Every purchase is a lot with its own date, so holding periods, FIFO sales and
XIRR are exact. Mutual funds are valued from AMFI's NAVs, and holdings in
different currencies are totalled in one (`--base`). `rebalance --inflow` puts
new money only into underweight holdings.

### Capital-gains tax

```bash
stockwise tax in                                  # LTCG/STCG per lot, with set-off rules
stockwise harvest                                 # losses and gains to book before 31 March
stockwise tax advance --paid 25000                # advance-tax instalments and Sec. 234C interest
stockwise tax export in --fy 2025-26 > cg.csv     # Schedule CG / 112A rows for your ITR
```

Check the [known issues](CHANGELOG.md) for the current release before you file
with these numbers.

### Look things up

```bash
stockwise quote RELIANCE TCS                      # Indian stocks are the default
stockwise quote AAPL NVDA -m us                   # US stocks; also crypto, FX and 7 global exchanges
stockwise TCS                                     # no command needed: a one-screen overview
stockwise deep HDFCBANK                           # fundamentals, technicals and risk for one stock
stockwise score INFY --explain                    # Piotroski F-score and Altman Z-score, with the working
stockwise events TCS                              # filings and news timeline, and how the stock moved
stockwise insights TCS                            # findings on ownership, growth, margins and valuation
stockwise mf search parag flexi direct            # mutual funds from AMFI; `mf nav CODE` adds the expense ratio
stockwise options chain NIFTY                     # option chain with IV and greeks
stockwise options oi NIFTY                        # open-interest build-up, PCR and max pain
stockwise ipo                                     # open and upcoming IPOs from NSE
stockwise markets                                 # indices at a glance
```

`q`, `d`, `a`, `t`, `c`, `p` and `w` are short for `quote`, `dashboard`,
`analyze`, `technical`, `compare`, `portfolio` and `watch`. `--explain` on
`technical`, `score` and `options oi` adds plain-English notes for beginners.

Many commands take `--json`, and tables take `--csv` (`stockwise <command> --help`
shows which), so they fit in scripts:
`stockwise portfolio --json | jq '.totals.pnl_percent'`.

### Ask an AI about your money

`stockwise mcp` is an [MCP](https://modelcontextprotocol.io) server: Claude
Desktop, Claude Code, Cursor and other assistants can look up quotes,
technicals, risk, scorecards, events, mutual funds, option chains, IPOs, screens
and backtests, and read your portfolio, lots, tax estimate, advance tax,
harvest and rebalance plans, calendar, watchlist and alerts — 34 read-only
tools, three resources and three ready-made prompts ("review my portfolio",
"tax-harvest check before 31 March", "explain this option chain"). It never
places orders. `stockwise ask "question"` lets a local or cloud model call the
same tools from your terminal.

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

<!-- mcp-name: io.github.iamsaquib8/stockwise -->

## Also included

- **Backtests that count Indian costs** — STT, brokerage, stamp duty, next-day
  fills, walk-forward tests; strategies as small TOML files.
  [Backtesting](docs/backtesting.md)
- **Practice on past markets** — replay the 2020 crash with the future hidden,
  scored on risk, not raw return; a paper-trading simulator.
  [Simulator](docs/simulator.md)
- **Dashboards** — `stockwise tui` in the terminal and `stockwise serve --open`
  in the browser; neither places orders. [TUI](docs/tui.md), [web](docs/server.md)
- **Alerts** — price, RSI, moving-average, breakout, volume and results alerts to
  Telegram, Slack, Discord or email (desktop pop-ups need a source build with
  `--features desktop-notify`). [Alerts](docs/bots.md#notifications)
- **Planning** — SIP, retirement and education goals; SIP and what-if
  calculators. [Commands](docs/commands.md)
- **Gold, SGBs and G-secs** — gold in ₹/10 g, Sovereign Gold Bonds, bond yields
  and a YTM calculator. [Markets](docs/markets.md)
- **Live orders (optional)** — Angel One; Zerodha, Upstox, Dhan and Fyers are
  experimental and untested on a live account, as are stop-loss and GTT orders.
  Every order shows a preview and needs a typed confirmation. [Brokers](docs/brokers.md)

The [cheatsheet](docs/cheatsheet.md) has the commands people use most, and the
[commands reference](docs/commands.md) has every option, including the
[configuration file](docs/commands.md#configuration-file).

## Docs

**[iamsaquib8.github.io/stockwise](https://iamsaquib8.github.io/stockwise/)** —
install, cheatsheet, commands, markets, AI, backtesting, brokers, and
[how StockWise compares](docs/comparison.md) with other tools.

## Contributing

Bug reports and ideas are welcome in
[Issues](https://github.com/iamsaquib8/stockwise/issues) and
[Discussions](https://github.com/iamsaquib8/stockwise/discussions). The
[roadmap](ROADMAP.md) shows what is planned up to 1.0. Start with
[CONTRIBUTING.md](CONTRIBUTING.md); a
[good first issue](https://github.com/iamsaquib8/stockwise/labels/good%20first%20issue)
names the files to change and how to test.

## License

[MIT](LICENSE) © iamsaquib8. Market data comes from Yahoo Finance, AMFI, the
exchanges' public pages and your broker. StockWise is not affiliated with any
exchange, broker or data provider. See [DISCLAIMER.md](DISCLAIMER.md).
