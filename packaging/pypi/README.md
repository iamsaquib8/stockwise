# StockWise

**A stock market toolkit for your terminal, built for Indian (NSE/BSE) and US
stocks.** Quotes, charts, screeners, backtests, SIP math, paper trading and
portfolio tracking in one fast binary. No API keys, no account, no browser tabs.

This package installs the prebuilt `stockwise` command-line tool. It has no
Python API and no Python dependencies; the wheel carries the compiled binary
for your platform.

```bash
pipx install stockwise        # or: uv tool install stockwise
stockwise doctor              # check your setup
```

Or run it once without installing:

```bash
uvx stockwise quote RELIANCE TCS
```

## A taste

```bash
stockwise quote RELIANCE TCS            # Indian stocks are the default
stockwise quote AAPL NVDA -m us         # US stocks
stockwise deep HDFCBANK                 # everything about one stock
stockwise backtest INFY -s macd         # test a strategy, after STT and brokerage
stockwise sip NIFTYBEES 5000 -p 5y      # what a ₹5,000/month SIP became (XIRR)
stockwise mf search parag flexi direct  # mutual funds from AMFI
stockwise tui                           # live dashboard in the terminal
```

Mutual funds, F&O option chains, crypto and FX, portfolio XIRR and Indian
capital-gains tax, alerts, a paper-trading simulator and optional live orders
through Indian brokers — see the
[documentation](https://iamsaquib8.github.io/stockwise/) and the
[cheatsheet](https://iamsaquib8.github.io/stockwise/cheatsheet.html).

Your portfolio stays in local files and StockWise sends no telemetry.

## Use with AI assistants

`stockwise mcp` is a Model Context Protocol server: Claude Desktop, Claude
Code, Cursor and other assistants can use quotes, fund NAVs, option chains,
backtests and your portfolio as read-only tools. It never places orders. With
`uvx` there is nothing to install first:

```json
{ "mcpServers": { "stockwise": { "command": "uvx", "args": ["stockwise", "mcp"] } } }
```

See [AI](https://iamsaquib8.github.io/stockwise/ai.html) for setup and limits.

## Other ways to install

Homebrew, Scoop, winget, AUR, `.deb`/`.rpm`, Docker, Nix, Termux and
`cargo install stockwise` — see
[Install](https://github.com/iamsaquib8/stockwise#install).

> **Not financial advice.** StockWise is for education and information. See the
> [disclaimer](https://github.com/iamsaquib8/stockwise/blob/master/DISCLAIMER.md).

MIT licensed · [Source](https://github.com/iamsaquib8/stockwise) ·
[Changelog](https://github.com/iamsaquib8/stockwise/blob/master/CHANGELOG.md)

<!-- mcp-name: io.github.iamsaquib8/stockwise -->
