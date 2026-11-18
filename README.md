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

### New since 0.8.1 (unreleased, coming in 0.9)

- **Set up in one go** — `stockwise init` picks a market and imports a broker
  file; `stockwise profile set active:med` saves your investor profile and
  guardrails, which research, goals, rebalancing and `ask` all read.
- **Portfolio you can trust and fix** — `portfolio analyze` (by market,
  currency, cap tier and sector, short- vs long-term gains, risk),
  `portfolio pme` against an index twin, an `income` calendar with yield on
  cost, `sell --preview` with the tax before you sell, and lot fixes
  (`--qty`, `--price`, `--delete`, `--undo-sale`) with the last five versions
  kept for `portfolio restore`.
- **Deeper research** — `results` shows 8–12 quarters side by side (SEC EDGAR
  for US stocks), cap-tier badges everywhere, `conditions` for where a market
  stands, and scans of whole markets
  (`--universe all|bse|nifty500|midcap150|smallcap250`).
- **Funds and goals** — `mf bleed` prices the Regular-vs-Direct cost; `goal` is
  a horizon-aware, post-tax planner with outcome bands.
- **Runs in the background** — a SQLite `cache.db` and `stockwise worker` keep
  quotes, NAVs and snapshots fresh and send notices (lots turning long-term,
  new results, filings). Redis is opt-in.
- **AI, wired up** — `stockwise connect` adds StockWise to every AI app on your
  computer; `stockwise ask` remembers your profile and conversations and runs
  playbooks; assistants can change your workspace only with `--allow-writes`,
  and `stockwise undo` reverses it.

Full list: [CHANGELOG.md](CHANGELOG.md) and [changelog.d](changelog.d/).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/iamsaquib8/stockwise/main/install.sh | sh   # macOS, Linux, Termux
cargo binstall stockwise                                                                    # or: cargo install --locked stockwise
```

Also `.deb`/`.rpm`, Scoop, Nix and Docker — see [Install](docs/install.md) for
every option, and the [five-minute quickstart](docs/quickstart.md) for your
first commands. PyPI, Homebrew, winget and the AUR are not published yet. Building
from source needs Rust 1.97 or newer. Then run `stockwise init` to pick a market
and import your holdings (or `stockwise doctor` to check your setup). Switch the
default market later with `stockwise use us` (or `uk`, `crypto`, ...), so you don't
need `-m` on every command.

## What it does

### One portfolio, true returns

```bash
stockwise import kite ~/Downloads/tradebook.xlsx  # CSV or .xlsx from Kite, Groww, Upstox, INDmoney; CAS too
stockwise portfolio                               # live P&L, XIRR and allocation
stockwise portfolio sell TCS 5 3900 --preview     # lots taken and tax due, before you record it
stockwise portfolio sell TCS 5 3900               # FIFO sale; realised P&L kept for tax
stockwise portfolio analyze                       # by market, currency, cap and sector; risk and tax
stockwise portfolio lots TCS --lot 2 --qty 10     # fix a lot (--price, --date, --delete, --undo-sale N)
stockwise portfolio benchmark NIFTY -p 1y         # return vs the index
stockwise portfolio xray                          # real exposure across stocks, funds and ETFs
stockwise portfolio target set TCS 30             # target weights; then rebalance --inflow 50000
```

Every purchase is a lot with its own date, so holding periods, FIFO sales and
XIRR are exact. Mutual funds are valued from AMFI's NAVs, and holdings in
different currencies are totalled in one (`--base`). `rebalance --inflow` puts
new money only into underweight holdings. A tradebook import keeps its trades and
merges with the next file, so yearly Console downloads can be loaded one by one,
in any order. Every change keeps the previous version (`portfolio.json.1` to
`.5`): `portfolio restore` lists them and `portfolio restore N` brings one back. More: [portfolio and imports](docs/commands.md#portfolio),
[X-Ray and benchmark](docs/commands.md#benchmark-and-attribution).

### Capital-gains tax

```bash
stockwise tax in                                  # LTCG/STCG per lot, with set-off rules
stockwise harvest                                 # losses and gains to book before 31 March, by FIFO sale
stockwise tax advance --paid 2025-06-14:3000      # dated payments: Sec. 234C interest by due date
stockwise tax export in --fy 2025-26 > cg.csv     # Schedule CG / 112A rows for your ITR
```

Tax is worked out per lot, with the 4% cess and the rates in force on each sale
date (sales before 23 July 2024 use the old ones). The numbers cover only what
StockWise has recorded, so compare them with your broker's tax P&L before you
file. More: [how tax is worked out](docs/commands.md#lots-sales-and-tax),
[harvest](docs/commands.md#harvest), [tax export](docs/commands.md#tax-export),
[advance tax](docs/commands.md#advance-tax-india).

### Look things up

```bash
stockwise quote RELIANCE TCS                      # Indian stocks are the default
stockwise quote AAPL NVDA -m us                   # US stocks; also crypto, FX and 7 global exchanges
stockwise quote NVDA                              # a ticker only one market lists needs no -m
stockwise use us                                  # make US stocks the default; `stockwise use` shows it
stockwise TCS                                     # no command needed: a one-screen overview
stockwise deep HDFCBANK                           # fundamentals, technicals and risk for one stock
stockwise score INFY --explain                    # Piotroski F-score and Altman Z-score, with the working
stockwise events TCS                              # filings and news timeline, and how the stock moved
stockwise insights TCS                            # findings on ownership, growth, margins and valuation
stockwise results INFY                            # 8-12 quarters side by side, with YoY, QoQ and TTM
stockwise conditions                              # index vs its averages, VIX, breadth and valuation
stockwise mf search parag flexi direct            # mutual funds from AMFI; `mf nav CODE` adds the expense ratio
stockwise options chain NIFTY                     # option chain with IV and greeks
stockwise options oi NIFTY                        # open-interest build-up, PCR and max pain
stockwise ipo                                     # open and upcoming IPOs from NSE
stockwise markets                                 # indices at a glance; --fx adds INR remittance pairs
```

`q`, `d`, `a`, `t`, `c`, `p` and `w` are short for `quote`, `dashboard`,
`analyze`, `technical`, `compare`, `portfolio` and `watch`. `--explain` on
`verdict`, `technical`, `score` and `options oi` adds plain-English notes for
beginners. All commands: [commands reference](docs/commands.md).

Many commands take `--json`, and tables take `--csv` (`stockwise <command> --help`
shows which), so they fit in scripts:
`stockwise portfolio --json | jq '.totals.pnl_percent'`.

### Ask an AI about your money

`stockwise mcp` is an [MCP](https://modelcontextprotocol.io) server: Claude,
ChatGPT Codex, Gemini CLI, Google Antigravity, Cursor, VS Code and other
assistants can look up quotes, technicals, risk, scorecards, events, mutual
funds, option chains, IPOs, screens, picks, market conditions and backtests,
and read your portfolio dashboard, lots, tax estimate, advance tax, harvest and
rebalance plans, calendar, goals, watchlist and alerts — 52 tools that read,
three resources and four ready-made prompts. `fit_check` answers "does this
stock fit my own rules?" with yes, no or unclear. With `--allow-writes` the
assistant can also set up your profile and change your watchlist, alerts,
goals and notes; `stockwise undo` reverses any change. It never places orders.

`stockwise connect` adds it to every AI app on your computer in one step.
`stockwise ask "question"` lets a local or cloud model call the same tools
from your terminal, with your profile, saved conversations (`-c`, `-i`) and
playbooks (`--playbook checkup`).

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

Over HTTP: `stockwise mcp --http 127.0.0.1:8788`. Agents can also read the
[Agent Skill](skills/stockwise/SKILL.md) and the docs'
[llms.txt](https://iamsaquib8.github.io/stockwise/llms.txt). Details, the tool
list and the safety model: [docs/ai.md](docs/ai.md#use-stockwise-from-ai-assistants-mcp).

<!-- mcp-name: io.github.iamsaquib8/stockwise -->

## Also included

- **Background worker and cache** — `stockwise worker start` keeps a local
  SQLite `cache.db` warm and sends post-close notices; `stockwise cache status`
  shows what is stored. [Commands](docs/commands.md)
- **Income and goals** — `stockwise income` projects dividends and coupons for
  12 months; `stockwise goal` plans SIPs by horizon, after tax.

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
install, a [five-minute quickstart](docs/quickstart.md), guides by goal,
and the reference.

New here? [What do you want to do?](docs/guides.md) lists the guides:

- [Import your trades and see true returns](docs/guide-portfolio.md)
- [File your Indian capital-gains tax](docs/guide-tax.md)
- [Check a stock or fund fast](docs/guide-research.md)
- [Rebalance to target weights](docs/guide-rebalance.md)
- [Use StockWise from Claude, Gemini, Codex or Cursor](docs/guide-mcp.md)
- [Set up alerts and the daemon](docs/guide-alerts.md)
- [Backtest and practise on past markets](docs/guide-practice.md)
- [Script it with JSON, CSV and jq](docs/guide-scripting.md)

The reference has the [cheatsheet](docs/cheatsheet.md), the
[commands](docs/commands.md), markets, AI, backtesting, brokers,
[troubleshooting](docs/troubleshooting.md), and
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
