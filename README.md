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
markets.** You keep your holdings in local files. StockWise reviews them with
published rules, shows what moved and why, ranks candidates, and works out
your capital-gains tax. One binary, no account, no API key, no telemetry.

> **Not financial advice.** Actions are rule-based educational signals.
> StockWise is not a SEBI-registered investment adviser or research analyst.
> You can lose money. See [DISCLAIMER.md](DISCLAIMER.md).

The examples below are real output from a sample portfolio of six large caps
on 21 November 2026, trimmed. Your output differs.

## What should I do with my holdings?

```bash
stockwise portfolio review
```

```text
  Portfolio review (long horizon)
  2 exit · 3 trim · 0 add · 1 hold · 0 no call   (long horizon, methodology 1)

  INFY.NS  INFOSYS LIMITED  9.0% of portfolio  P&L -26.3%
    ACTION: EXIT · medium confidence · review by 13 Jan 2027
      • Closed at ₹1,098.00 below the invalidation level ₹1,179.91

  RELIANCE.NS  RELIANCE INDUSTRIES LTD  30.2% of portfolio  P&L +133.5%
    ACTION: TRIM · medium confidence · invalid below ₹1,322.75 · review by 15 Jan 2027
      • Weight 30.2% is above the 12.0% cap for one stock
      • Size: sell about 49 shares to get back to 12.0% of the portfolio (max_stock_weight)

  LT.NS  LARSEN & TOUBRO LTD.  11.5% of portfolio  P&L +27.6%
    ACTION: HOLD · high confidence · invalid below ₹4,011.62 · review by 28 Jan 2027
      • Composite score 72: no exit, trim or add rule triggered
      • Trend not up (price and 50-day average not both above the 200-day), so no add
      • Price ₹4,209.10 is above the invalidation level ₹4,011.62

  Rule-based educational signal from StockWise's published methodology, not investment
  advice. StockWise is not a SEBI-registered investment adviser or research analyst. ...
```

Each holding gets ADD, HOLD, TRIM or EXIT, or NO CALL when less than 60% of
the data is there. Each action shows up to three reasons, a confidence level,
an invalidation price and a review date. `--horizon short` reviews for weeks
to months. `stockwise verdict SYMBOL` shows the action and the 6-pillar score
for one stock. Every rule is in [methodology.md](docs/methodology.md#actions-methodology-1).
Guide: [Review your holdings with rule-based actions](docs/guide-actions.md).

## What's hot in my investments?

```bash
stockwise hot
```

```text
  What's hot — Sat 21 Nov 20:00 IST · India

  Your investments — hottest first
  ────────────────────────────────────────
  ITC.NS            -0.6%  heat  6.8  DGGI Guwahati | Can GST Dept. Impose Separate S. 122 Penalty …
                 1.3σ · 16.4% of portfolio
  RELIANCE.NS       +0.1%  heat  5.9  little changed, no news …
                 30.2% of portfolio
  LT.NS             -0.0%  heat  3.9  NCLT Mumbai Dispenses With L&T Shareholder, Creditor Meetings …
                 1.0σ · 11.5% of portfolio

  Sectors
  ────────────────────────────────────────
  Auto                    +5.1%
  Bank                    +4.2%
  PSU Bank                +3.5%
```

Heat combines the move against the index (in σ), the volume surge, fresh news
or filings and a date within three days, scaled by portfolio weight. Then come
the market lists: biggest moves, unusual volume, gaps, 52-week highs and lows,
sectors, themes, FII/DII flows and NSE bulk and block deals. `stockwise today`
explains each holding's day move, and `stockwise digest` is the after-close
summary (the background worker can send it to Telegram, Slack, email and
more). Guide: [See what's hot and what changed today](docs/guide-daily.md).

## Find candidates

```bash
stockwise picks --universe nifty50 --top 5
```

```text
  Highest Scores, long term — NIFTY 50
  ⟳ 50 stocks ranked against each other on value, quality, growth, trend and risk

  Symbol              Price     Chg%  Score  Value  Quality  Growth  Trend  Risk  Data  Action       Top fifth on
  ───────────────────────────────────────────────────────────────────────────────────────────────────────────────────
  STOCK-A.NS        ₹•••.••   -0.38%     71     94       77      42     54    77  100%  WATCH (med)  Value
  STOCK-B.NS      ₹•,•••.••   +1.93%     66     63       67      37     79    88   95%  BUY (high)   Risk
  STOCK-C.NS      ₹•,•••.••   +1.35%     65     59       60      62     93    51  100%  WATCH (high) Trend
  Each number is a percentile rank among the stocks scanned (100 is the highest). ...
  Rule-based educational signal from StockWise's published methodology, not investment advice. ...
```

Symbols and prices are hidden here: this README is not a list of stocks to
buy. `picks` ranks the stocks it scans against each other, then applies the
action rules to each one as if you did not hold it. `--horizon short` weights
momentum, volume and risk for weeks to months; `--cap small` ranks small caps
against each other with their own floors. Every list is logged, and
`stockwise picks review` shows how past lists did against the index.
Guide: [Find candidates with picks](docs/guide-picks.md).

## File my tax

```bash
stockwise tax in
```

```text
  Tax Estimate — India (FY 2026-27)

  Realized — sales 01 Apr 2026 to 31 Mar 2027
  ────────────────────────────────────────
  Symbol               Qty Bought       Sold                 Gain  Term
  RELIANCE.NS        20.00 2024-03-12   2026-10-15        +₹3,800    LT
  ITC.NS             50.00 2025-12-02   2026-09-10        -₹3,750    ST

  Estimated tax on gains realized in FY 2026-27
  ────────────────────────────────────────
  ST loss set off vs LTCG      ₹3,750.00
  LTCG after set-off           ₹50.00
  LTCG exemption (₹1,25,000 a year) -₹50.00
  TOTAL TAX                    ₹0.00

  → ₹1,24,950 of this year's ₹1,25,000 LTCG exemption is unused — booking up to that much long-term gain this FY is tax-free.
```

Tax is worked out per lot, with the set-off rules, the 4% cess and the rates
in force on each sale date. `stockwise tax reconcile taxpnl.xlsx` matches
each sale with your broker's tax P&L (Zerodha, Groww, Upstox) and explains
every difference. `stockwise tax export in` writes the ITR Schedule CG / 112A
CSV, `stockwise harvest` lists losses and gains to book before 31 March, and
`stockwise rebalance --raise 100000` raises cash at the lowest tax. US
(Form 8949) and UK rules are included. Guide:
[File your Indian capital-gains tax](docs/guide-tax.md).

## Get started

```bash
stockwise init                                    # pick a market, import a broker file
stockwise import kite ~/Downloads/tradebook.xlsx  # or groww, upstox, indmoney, cas, csv
stockwise portfolio add TCS 10 3500 --date 2024-04-12
stockwise portfolio                               # live P&L, XIRR and an Action column
stockwise doctor                                  # check the setup; each failure has a fix
```

Indian stocks are the default (`TCS` means `TCS.NS`). Add `-m us` for US
stocks, or run `stockwise use us` once. Crypto, FX, gold and seven more
exchanges work too ([markets](docs/markets.md)).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/iamsaquib8/stockwise/main/install.sh | sh   # macOS, Linux, Termux
```

| Channel | Command |
|---------|---------|
| Shell installer | the line above (checks the SHA-256; `stockwise update` updates) |
| Cargo | `cargo binstall stockwise` or `cargo install --locked stockwise` (Rust 1.97+) |
| Debian, Ubuntu | `stockwise_amd64.deb` / `stockwise_arm64.deb` from the [latest release](https://github.com/iamsaquib8/stockwise/releases/latest) |
| Fedora, RHEL | `sudo dnf install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise-x86_64.rpm` |
| Windows (Scoop) | `scoop install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise.json` |
| Nix | `nix profile install github:iamsaquib8/stockwise` |
| Docker | `docker run --rm -it -v stockwise-data:/data ghcr.io/iamsaquib8/stockwise quote TCS` |
| Release binaries | archives for macOS, Linux, Windows and Android from the [latest release](https://github.com/iamsaquib8/stockwise/releases/latest) |

Prepared but not published yet: PyPI (`pipx install stockwise`), Homebrew,
winget, the AUR, Docker Hub, nixpkgs and Termux packages.
[Install](docs/install.md) has the details.

## Use it from an AI assistant

```bash
stockwise connect          # adds the MCP server to every AI app found on this computer
```

That covers Claude Desktop, Claude Code, ChatGPT Codex, Gemini CLI,
Antigravity, Cursor, VS Code and more (`stockwise connect --list`). For Claude
Code alone: `claude mcp add stockwise -- stockwise mcp`. The server has 71
tools that read; the assistant explains StockWise's computed actions and never
invents one. With `--allow-writes` it can change your watchlist, alerts,
goals and notes, and `stockwise undo` reverses any change. It never places
orders. `stockwise ask "question"` uses the same tools from your terminal.
Guide: [Use StockWise from Claude, Gemini, Codex or Cursor](docs/guide-mcp.md).

<!-- mcp-name: io.github.iamsaquib8/stockwise -->

## More

| Topic | Where |
|-------|-------|
| Five-minute start | [Quickstart](docs/quickstart.md) |
| Guides by goal | [What do you want to do?](docs/guides.md) |
| Commands people use most | [Cheatsheet](docs/cheatsheet.md) |
| Every command and option | [Commands reference](docs/commands.md) |
| How actions, scores and picks work | [Methodology](docs/methodology.md) |
| Portfolio, lots and imports | [Portfolio guide](docs/guide-portfolio.md), [brokers](docs/brokers.md) |
| Rebalance to targets | [Rebalance guide](docs/guide-rebalance.md) |
| Alerts, worker and notifications | [Alerts guide](docs/guide-alerts.md), [bots](docs/bots.md) |
| Backtests and practice | [Backtesting](docs/backtesting.md), [simulator](docs/simulator.md) |
| Terminal and web dashboards | [TUI](docs/tui.md), [web and JSON API](docs/server.md) |
| AI and MCP | [AI](docs/ai.md) |
| Scripting with JSON and CSV | [Scripting guide](docs/guide-scripting.md) |
| How it is built | [Architecture](docs/architecture.md) |
| Something broke | [Troubleshooting](docs/troubleshooting.md) |
| Other tools | [Comparison](docs/comparison.md) |
| What changed | [CHANGELOG.md](CHANGELOG.md), [roadmap](ROADMAP.md) |

The docs site is **[iamsaquib8.github.io/stockwise](https://iamsaquib8.github.io/stockwise/)**.

## Disclaimer

StockWise gives rule-based educational signals, not personalised financial,
investment, tax or legal advice. It is not a SEBI-registered investment
adviser or research analyst. Data comes from third parties and can be late
or wrong. Check the reasons and the data before you act; every decision is
yours, and you can lose money. Live trading commands preview the order and ask
for typed confirmation by default; passing `--yes` when `max_order_value` is
set in your broker config bypasses typed confirmation and sends the order
immediately (so typed confirmation is not a safety check in that case). Read
[DISCLAIMER.md](DISCLAIMER.md).

## Contributing

Bug reports and ideas are welcome in
[Issues](https://github.com/iamsaquib8/stockwise/issues) and
[Discussions](https://github.com/iamsaquib8/stockwise/discussions). Start with
[CONTRIBUTING.md](CONTRIBUTING.md); a
[good first issue](https://github.com/iamsaquib8/stockwise/labels/good%20first%20issue)
names the files to change and how to test.

## License

[MIT](LICENSE) © iamsaquib8. Market data comes from Yahoo Finance, AMFI, the
exchanges' public pages and your broker. StockWise is not affiliated with any
exchange, broker or data provider.
