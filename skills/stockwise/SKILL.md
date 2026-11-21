---
name: stockwise
description: Research and track stocks, funds, markets, portfolios, goals and taxes with StockWise (its MCP tools or the `stockwise` command line). Covers Indian (NSE/BSE) and US stocks plus London, Frankfurt, Tokyo, Hong Kong, Singapore, Toronto and Sydney, mutual funds and SIPs, F&O option chains, screens, backtests, market conditions, the user's portfolio and capital-gains tax, watchlist, alerts, goals and investor profile. Use when the user asks about stocks, indices, mutual funds, their portfolio, whether a stock fits their own rules, financial goals or investment tax.
license: MIT
---

# StockWise

StockWise is a local-first market toolkit. It reads public market data
(Yahoo Finance, NSE/BSE, AMFI, niftyindices) and the user's own files
(portfolio, watchlist, alerts, goals, profile, notes) on their computer.
Docs: https://iamsaquib8.github.io/stockwise/ (agents: https://iamsaquib8.github.io/stockwise/llms.txt).

## Pick the interface

1. **MCP tools.** If tools such as `quote`, `portfolio_analyze`, `fit_check`
   from a `stockwise` server are available, use them. They take checked JSON
   arguments and return compact JSON.
2. **The CLI.** Otherwise run `stockwise` commands with `--json`. Check it
   with `stockwise --version`. If it is missing, tell the user how to install
   it (`cargo binstall stockwise`, the installer on the docs site, or a release
   package) instead of installing it yourself.

To connect StockWise to an AI app, the user runs `stockwise connect` (every
app found on the computer) or names apps: `stockwise connect antigravity cursor`.
`stockwise connect --list` shows Claude Desktop, Claude Code, Cursor, Windsurf,
VS Code, Cline, Gemini CLI, Antigravity, Codex, Zed, LM Studio, opencode,
Goose and Continue.

## What the tools may change

There are three tiers. The server's mode decides which tools you have.

| Tier | Tools | Turned on by |
|------|-------|--------------|
| Read | everything else | always |
| Workspace | `watchlist_add/remove`, `alert_add/remove`, `goal_save/remove`, `target_set/remove`, `profile_set`, `profile_onboard`, `note_set` | `stockwise mcp --allow-writes` |
| Portfolio | `portfolio_add`, `portfolio_sell` (record lots and sales, never orders) | `stockwise mcp --allow-portfolio` |

- Each change is recorded. The result holds `change_id` and an `undo`
  command; tell the user both. `changes_list` and `stockwise undo --list`
  show the journal; `stockwise undo` reverses the newest change.
- Change only what the user asked for. `portfolio_add` and `portfolio_sell`
  first return a preview; show it (symbol, quantity, price, date, gain) and
  call again with `confirm: true` only after the user agrees.
- Orders: `order_preview` only shows an order. Never place one. With the CLI,
  `trade` commands send real orders through the user's broker: show the
  command with `--dry-run` and let the user run the real one; never add `--yes`.

## Workflows

**First use: set up the profile.** If `profile` shows defaults (or the user
asks), call `profile_questions`, ask the questions one at a time in plain
words, then send the answers to `profile_onboard`. The mapping from answers to
settings is fixed in StockWise; report the reasons it returns. Without write
tools, show the user `stockwise profile set --help` instead.

**"How is my portfolio doing?"** `portfolio_analyze` (or `stockwise portfolio analyze --json`)
gives value, P&L, XIRR, split by market, currency, cap tier and sector, tax
terms, risk and observations. Add `portfolio_summary` for live P&L, and
`tax_estimate` or `harvest` for tax questions.

**"Should I buy X?" or "Is X good for me?"** Never make up a buy, sell or
hold call yourself: actions (BUY/WATCH/AVOID, ADD/HOLD/TRIM/EXIT) come only
from StockWise's rule-based engine, and this version has no action tool yet
(`stock_action` and `portfolio_review` are coming). Run `verdict` for the
6-pillar scores and their data coverage, `fit_check` with the symbol (and
`amount` if the user named one), and `risk`. `fit_check` answers
`fits_your_rules`: yes, no or unclear, against the user's own limits (largest
weight of one stock, small-cap share, risk level, and the P/E and ROE limits
in their note for that stock). Present it as "by your own rules", list the
failed and unknown rules, and lay out what the data shows for and against.
When the verdict's `data_coverage` is under 60, say there is not enough data
for a call (NO CALL). Offer `note_set` to record the user's thesis and limits
for next time. The MCP prompt `should_i_buy` does all of this.

**"Is it a good time to invest?"** `market_conditions` (or `stockwise conditions --json`)
shows trend, volatility, breadth and valuation with their history. These are
readings, not a timing signal; say so.

**Ideas to research.** `picks` ranks stocks that clear size and liquidity
floors on five pillars (value, quality, growth, trend, risk) among the stocks
scanned. Options: `universe` (such as `nifty50` or `smallcap250`), `cap`
(`small` ranks small caps against each other), `horizon` (`long`, the
default, or `short` for weeks to months), `min_mcap` and `min_adv`. Leave out
picks whose `coverage` is under 0.6. Every list is logged, and `picks_review`
shows how earlier lists did against the index: cite it, misses included.
`verdicts` scores up to 25 tickers in one call, and `quote` prices up to 25.
`screen`, `movers` and `ipo_board` help too. Each is a starting list for
research, not a recommendation.

**Watchlist and alerts upkeep.** `watchlist` and `alerts_list` read;
`watchlist_add`, `alert_add` (price levels or conditions such as RSI below 30)
and their `_remove` pairs change them.

**Goals.** `goal_sip`, `goal_retire`, `goal_education`, `goal_swp`,
`goal_prepay` and `goal_emergency` calculate. `goal_save` keeps a goal, and
`goal_list` shows each saved goal's funded share.

**Tax season (India).** `tax_estimate` for capital gains, `tax_advance` for
the advance-tax schedule, `harvest` for loss and 112A harvesting. With the CLI,
`stockwise tax export in --fy 2025-26 --json` gives Schedule CG rows.

**Indian fixed income, gold and currency.** `bonds_yields` (G-sec curve),
`bonds_list` and `bonds_calc` (price, yield, duration of a G-sec or T-bill),
`commodity` (gold and silver in rupees; `view: sgb` for Sovereign Gold Bonds
against gold), `fx_history` (USD/INR and other pairs). `returns`, `correlate`
and `compare` work for any ticker; `portfolio_wrap` with `fy: 2025-26`
reviews an Indian financial year. Prompts `mf_review` and
`advance_tax_check` cover funds and advance tax.

**Results season.** `calendar` and `income_calendar` for dates,
`quarterly_results` and `events` for what was reported.

**MCP extras.** Built-in prompts: `daily_briefing`, `should_i_buy`,
`exit_check`, `smallcap_short_scan`, `tax_aware_rebalance`, `weekly_review`,
`mf_review`, `advance_tax_check`, `review_portfolio`, `tax_harvest_check`,
`explain_option_chain`, `onboard_profile`. Resources: `stockwise://portfolio`, `watchlist`, `alerts`,
`profile`, `picks/latest`, `track-record`, `methodology`, and
`stockwise://quote/{symbol}`. The main tools also return `structuredContent`
that follows their `outputSchema`.

## CLI quick reference

- Add `--json` to data commands. stdout then holds only JSON; messages go to
  stderr and failures exit with code 1. Tables also take `--csv`.
- Bare tickers are Indian: `TCS` means `TCS.NS`. Use `-m us` for US stocks,
  `-m crypto`, `-m fx`, `-m commodity`, or an exchange: `-m uk`, `-m de`,
  `-m jp`, `-m hk`, `-m sg`, `-m ca`, `-m au`. A suffix (`TCS.NS`,
  `500325.BO`) is used as given.
- Periods: `-p 1d|5d|1mo|3mo|6mo|ytd|1y|2y|5y|10y|max`.
- Not sure of a ticker? `stockwise search "tata motors" --json` first.

| Need | Command |
|------|---------|
| Quotes | `stockwise quote RELIANCE TCS --json` · `stockwise quote AAPL NVDA -m us --json` |
| Indices | `stockwise markets --json` |
| Market conditions | `stockwise conditions --json` |
| Price history | `stockwise history INFY -p 1y --json` |
| RSI, MACD, moving averages | `stockwise technical TCS -p 6mo --json` |
| Volatility, Sharpe, drawdown | `stockwise risk HDFCBANK -p 1y --json` |
| 6-pillar score (not a rating) | `stockwise verdict TCS --json` |
| Piotroski and Altman | `stockwise scorecard TCS --json` |
| Ranked ideas | `stockwise picks --json` · `stockwise picks --cap small --horizon short --json` |
| Track record of picks | `stockwise picks review --json` |
| Gainers and losers | `stockwise movers --json` |
| Preset screen | `stockwise screen growth --json` |
| Backtest | `stockwise backtest INFY -s macd -p 2y --json` |
| Mutual funds | `stockwise mf search parag flexi direct --json` · `stockwise mf sip 122639 5000 -p 5y --json` |
| Options | `stockwise options chain NIFTY --json` · `stockwise options pcr NIFTY --json` |
| Portfolio dashboard | `stockwise portfolio analyze --json` |
| Holdings | `stockwise portfolio --json` · `stockwise portfolio xray --json` |
| Against an index | `stockwise portfolio benchmark NIFTY -p 1y --json` |
| Capital gains | `stockwise tax in --json` · `stockwise harvest --json` |
| Goals | `stockwise goal sip --target 1cr --years 15 --json` · `stockwise goal list --json` |
| Watchlist, alerts | `stockwise watch --json` · `stockwise alert check --json` |
| Profile | `stockwise profile show --json` |
| Assistant changes | `stockwise undo --list` |
| Setup problems | `stockwise doctor` |

`stockwise <command> --help` lists every option. `stockwise ask "…"` lets the
user's own configured model call the tools; suggest it, since it may send data
to a cloud model.

## Rules

- **Recommendations policy.** Actions come only from StockWise's
  deterministic engine; never invent, upgrade or soften one, and never turn
  scores into one. Explain with the reasons and figures a tool returned, and
  never invent price levels (entry, stop, target, invalidation). When data
  coverage is under 60%, say there is not enough data for a call. `verdict`,
  `scorecard`, `picks` and `fit_check` describe data or the user's own rules;
  they are not ratings.
- **Not personalised advice.** Say the information is educational, StockWise
  is not a SEBI-registered adviser, and quotes can be delayed (NSE quotes from
  Yahoo are about 15 minutes behind).
- **Tax figures are estimates.** They cover only the sales and lots
  StockWise has recorded. Tell the user to check them against their broker's
  tax P&L before filing.
- **Missing data stays missing.** Holdings without a price are listed in
  `unpriced` and unknown quotes in `missing`; report them, never treat them
  as zero. Report `unknown` rules from `fit_check` as unknown.
- **The CLI changes files directly.** `portfolio add/sell/remove`, `import`,
  `watch add/remove`, `alert add/remove`, `goal add/remove`, `profile set`
  are not journalled for undo. Run them only when the user asked for that
  change; prefer the MCP write tools, which are.
- Leave `--ai` off: it is slow and may send data to a cloud model.
- To try commands without touching the user's files, set
  `STOCKWISE_HOME=$(mktemp -d)`.

## Examples

```bash
# How are the markets and the user's holdings today?
stockwise markets --json
stockwise portfolio analyze --json

# Is INFY overbought, and how volatile has it been?
stockwise technical INFY --json
stockwise risk INFY -p 1y --json

# What would a ₹5,000 monthly SIP in Parag Parikh Flexi Cap have become?
stockwise mf search parag flexi direct growth --json
stockwise mf sip 122639 5000 -p 5y --json
```
