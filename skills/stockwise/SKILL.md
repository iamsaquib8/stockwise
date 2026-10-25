---
name: stockwise
description: Look up Indian (NSE/BSE) and US stock quotes, price history, technical indicators, risk, mutual funds (AMFI NAVs, SIP returns), F&O option chains, screens and backtests, and the user's own StockWise portfolio, lots, capital-gains estimate, watchlist and alerts, with the `stockwise` command-line tool or its MCP server. Use when the user asks about Indian or US stocks, NIFTY/SENSEX, mutual funds, SIPs, options, goal planning, or their StockWise portfolio and tax.
license: MIT
---

# StockWise

StockWise is a stock-market toolkit for the terminal, built for Indian (NSE/BSE)
and US stocks. It reads public market data (Yahoo Finance, NSE/BSE, AMFI) and
the user's local portfolio files. Docs: https://iamsaquib8.github.io/stockwise/
(agents: https://iamsaquib8.github.io/stockwise/llms.txt).

## Pick the interface

1. **MCP tools** — if tools named `quote`, `technical`, `portfolio_summary`,
   `tax_estimate` … from a `stockwise` server are available, use them. They
   are read-only, take checked JSON arguments and return compact JSON.
2. **The CLI** — otherwise run `stockwise` commands with `--json`. Check that
   it is installed with `stockwise --version`; if not, tell the user how to
   install it (`cargo binstall stockwise`, `pipx install stockwise`,
   `brew install iamsaquib8/tap/stockwise`, or the installer on the docs site)
   rather than installing it yourself.

To set up the MCP server for the user: `claude mcp add stockwise -- stockwise mcp`
(Claude Code), or command `stockwise` with args `["mcp"]` in other clients.

## Use the CLI the machine-readable way

- Add `--json` to every data command. stdout then holds only JSON; messages go
  to stderr and failures exit with code 1. Tables also take `--csv`.
- Bare tickers are Indian: `TCS` means `TCS.NS`. Use `-m us` for US stocks,
  `-m crypto`, `-m fx`, `-m commodity`, or an exchange: `-m uk`, `-m de`,
  `-m jp`, `-m hk`, `-m sg`, `-m ca`, `-m au`. A suffix (`TCS.NS`, `500325.BO`)
  is used as given.
- Periods: `-p 1d|5d|1mo|3mo|6mo|ytd|1y|2y|5y|10y|max`.
- Not sure of a ticker? `stockwise search "tata motors" --json` first.

| Need | Command |
|------|---------|
| Quotes | `stockwise quote RELIANCE TCS --json` |
| US quotes | `stockwise quote AAPL NVDA -m us --json` |
| Indices (NIFTY, SENSEX, …) | `stockwise markets --json` |
| Price history | `stockwise history INFY -p 1y --json` |
| RSI, MACD, moving averages | `stockwise technical TCS -p 6mo --json` |
| Volatility, Sharpe, drawdown | `stockwise risk HDFCBANK -p 1y --json` |
| Preset screen | `stockwise screen growth --json` |
| Backtest a strategy | `stockwise backtest INFY -s macd -p 2y --json` |
| Find a mutual fund | `stockwise mf search parag flexi direct --json` |
| Fund NAV and expense ratio | `stockwise mf nav 122639 --json` |
| Past SIP returns (XIRR) | `stockwise mf sip 122639 5000 -p 5y --json` |
| Option chain with greeks | `stockwise options chain NIFTY --json` |
| Put/call ratio, max pain | `stockwise options pcr NIFTY --json` · `stockwise options maxpain NIFTY --json` |
| Strategy payoff | `stockwise options payoff NIFTY --strategy iron-condor --json` |
| The user's portfolio | `stockwise portfolio --json` |
| Capital-gains estimate | `stockwise tax in --json` |
| Results and dividend dates | `stockwise calendar --json` |
| Watchlist, alerts | `stockwise watch --json` · `stockwise alert check --json` |
| SIP for a goal | `stockwise goal sip --target 1cr --years 15 --json` |
| Retirement corpus | `stockwise goal retire --monthly-expense 80k --years-to-retire 20 --years-in-retirement 30 --json` |
| Setup problems | `stockwise doctor` |

`stockwise <command> --help` lists every option. For a question that needs
several of these, `stockwise ask "…"` lets the user's configured model call
them itself — only suggest it; it may send data to a cloud model.

## Rules

- **Read only.** Do not run commands that change the user's data —
  `portfolio add/sell/remove`, `import`, `watch add/remove`, `alert add/remove`,
  `config init --force`, `sim` trades — unless the user asked for that change.
- **Never place orders.** `trade buy`, `trade sell`, `trade limit` and
  `trade gtt` send real orders through the user's broker. Do not run them on your own initiative.
  If the user asks, show the command with `--dry-run` and let them run the
  real one; never add `--yes`.
- **Not financial advice.** Describe what the numbers show and their risks.
  Do not tell the user to buy, sell or hold, and say that prices can be
  delayed (NSE quotes from Yahoo are about 15 minutes behind).
- **Missing data stays missing.** Holdings without a price are listed in
  `unpriced` and quotes StockWise could not find in `missing`; report them,
  never treat them as zero.
- Leave `--ai` off: it is slow and may send data to a cloud model.
- To try commands without touching the user's files, set
  `STOCKWISE_HOME=$(mktemp -d)`.

## Examples

```bash
# How did NIFTY and the user's holdings do today?
stockwise markets --json
stockwise portfolio --json

# Is INFY overbought, and how volatile has it been?
stockwise technical INFY --json
stockwise risk INFY -p 1y --json

# What would a ₹5,000 monthly SIP in Parag Parikh Flexi Cap have become?
stockwise mf search parag flexi direct growth --json
stockwise mf sip 122639 5000 -p 5y --json
```
