# Cheatsheet

One page of the commands people use most. `stockwise <command> --help` shows
every option; the [commands reference](commands.md) explains each one.

For step-by-step walkthroughs, see [What do you want to do?](guides.md).

Bare tickers use the Indian market (`TCS` → `TCS.NS`). Add `-m us`, `-m uk`,
`-m jp`, `-m crypto`, `-m fx`, `-m commodity` … for other markets, or set
`STOCKWISE_MARKET`. Many commands take `--json`, and tables take
`--csv` (`stockwise <command> --help` shows which).

## Quotes and markets

| Do this | Command |
|---|---|
| Quote Indian stocks | `stockwise quote RELIANCE TCS` (or bare `stockwise RELIANCE`) |
| Quote US stocks | `stockwise quote AAPL NVDA -m us` |
| Everything about one stock | `stockwise deep HDFCBANK` |
| Indian and US indices | `stockwise markets` |
| One index per exchange worldwide | `stockwise markets --global` |
| Gainers and losers | `stockwise movers` |
| Sector performance | `stockwise sectors` |
| Find a ticker by name | `stockwise search "tata motors"` |
| First-run setup: market, holdings import, MCP snippet | `stockwise init` (no prompts without a TTY: `init --market in --import groww:FILE --no-mcp`) |
| Check your setup | `stockwise doctor` |
| Persistent response cache: size, clear | `stockwise cache status` / `stockwise cache clear` |
| Pre-load the market universe into `cache.db` | `stockwise cache warm` (or `--background`) |
| Filter cached stocks offline by cap, sector, P/E, RVOL | `stockwise cache snapshots --cap large --min-pe 10 --sort roe` |
| Run the background worker (live quotes, tasks, alerts, post-close sync) | `stockwise worker start` / `status` / `stop` |
| Queue a long job and follow it | `stockwise tasks add run -- backtest TCS` then `stockwise tasks` / `tasks show 1` |

## Analysis

| Do this | Command |
|---|---|
| Fundamentals in plain English | `stockwise analyze INFY` |
| Key events & filings timeline | `stockwise events LT` |
| Positive and negative findings | `stockwise insights LT` |
| 6-pillar score (not a recommendation) | `stockwise verdict LT` · `stockwise verdict LT --explain` |
| Piotroski F-score & Altman Z-score | `stockwise score INFY --explain` · `stockwise scorecard TCS --json` |
| Open and upcoming IPOs | `stockwise ipo` |
| One IPO: demand, scores and allotment odds | `stockwise ipo BCSSL --capital 2.5L` |
| RSI, MACD, Bollinger Bands | `stockwise technical TCS -p 6mo --explain` |
| Price chart with volume | `stockwise history AAPL -m us -p 1y` |
| Compare stocks | `stockwise compare TCS INFY WIPRO` |
| Volatility, Sharpe, drawdown | `stockwise risk HDFCBANK` |
| Support and resistance | `stockwise support RELIANCE` |
| Stop-loss levels | `stockwise stoploss TCS --entry 3500` |
| Preset screens | `stockwise screen growth` |
| Results and dividend dates | `stockwise calendar` |

## Funds, options, commodities, bonds

| Do this | Command |
|---|---|
| Find a mutual fund | `stockwise mf search parag flexi direct` |
| SIP returns (XIRR) for a fund | `stockwise mf sip 122639 5000 -p 5y` |
| Rank a category by return | `stockwise mf screen flexi cap` |
| Option chain with greeks | `stockwise options chain NIFTY` |
| Put/call ratio, max pain | `stockwise options pcr BANKNIFTY` · `stockwise options maxpain NIFTY` |
| OI build-up & resistance/support walls | `stockwise fno oi NIFTY` · `stockwise options oi BANKNIFTY -n 5 --explain` |
| Strategy payoff | `stockwise options payoff NIFTY --strategy iron-condor --width 200` |
| Gold and silver in ₹/10 g | `stockwise commodity spot` |
| Sovereign Gold Bonds | `stockwise commodity sgb` |
| G-sec and T-bill yields | `stockwise bonds yields` |

## Portfolio and tax

| Do this | Command |
|---|---|
| Add a purchase (a lot) | `stockwise portfolio add TCS 10 3500 --date 2023-04-12` |
| Sell (oldest lots first) | `stockwise portfolio sell TCS 5 3900` |
| Holdings, P&L and XIRR | `stockwise portfolio` |
| Track a mutual fund (AMFI code or ISIN, valued at its NAV) | `stockwise portfolio add MF:122639 100 80 --date 2024-01-10` |
| Portfolio X-Ray (overlap, concentration, sectors) | `stockwise portfolio xray` · `stockwise portfolio xray --json` |
| Import from your broker | `stockwise import kite holdings.xlsx` (CSV or `.xlsx`; also `groww`, `upstox`, `indmoney`, `cas`) |
| Rebuild a symbol's lots from a dated file | `stockwise import kite tradebook.csv --replace` |
| One holding's lots, sales and XIRR | `stockwise portfolio lots TCS` |
| Compare with NIFTY | `stockwise portfolio benchmark NIFTY -p 1y` |
| Splits, bonuses, dividends | `stockwise portfolio actions --apply` |
| Set target allocation | `stockwise portfolio target set TCS 25` · `stockwise portfolio target show` |
| Inflow rebalance (buy-only) | `stockwise rebalance --inflow 50000` (buy underweight without selling) |
| Rebalance with drift band | `stockwise rebalance --drift 2.0` |
| Tax-loss & LTCG exemption harvest | `stockwise harvest` · `stockwise harvest --json` |
| Capital-gains estimate | `stockwise tax in` |
| ITR Schedule CG CSV | `stockwise tax export in --out cg.csv` |
| Advance tax schedule & 234C estimator | `stockwise tax advance` · `stockwise tax advance --paid 25000` |
| Advance tax with each payment dated (exact 234C) | `stockwise tax advance --fy 2025-26 --paid 2025-06-14:3000 --paid 2025-09-10:5000` |
| Everything on one screen | `stockwise dashboard` |

## Alerts and watchlist

| Do this | Command |
|---|---|
| Watch stocks | `stockwise watch add INFY HDFCBANK` |
| Price alert | `stockwise alert add RELIANCE below 1200` |
| RSI alert | `stockwise alert add TCS rsi-below 30` |
| Moving-average cross | `stockwise alert add INFY cross-above-sma 200` |
| Breakout alert | `stockwise alert add SBIN breakout-high 52` |
| Keep checking during market hours | `stockwise alert watch` |
| Set up Telegram/Slack/email | `stockwise notify` |

## Planning

| Do this | Command |
|---|---|
| SIP needed for a target | `stockwise goal sip --target 1cr --years 15` |
| Retirement corpus and SIP | `stockwise goal retire --monthly-expense 80000 --years-to-retire 20 --years-in-retirement 30` |
| Education fund | `stockwise goal education --cost 25L --years 10` |
| Short goal, ₹10L in hand after tax | `stockwise goal sip --target 10L --months 9 --net` |
| Save a goal, track it | `stockwise goal add house --target 50L --by 2031-06` · `stockwise goal list` |
| How long a corpus lasts | `stockwise goal swp --corpus 1cr --withdrawal 50k` |
| Prepay a loan or invest | `stockwise goal prepay --principal 50L --loan-rate 9 --remaining-months 240 --extra 10k` |
| What a past SIP became | `stockwise sip NIFTYBEES 5000 -p 5y` |
| What a lump sum became | `stockwise whatif TCS 100000 2015-01-01` |

## Strategies and practice

| Do this | Command |
|---|---|
| Backtest a strategy (after costs) | `stockwise backtest INFY -s macd` |
| Backtest a basket | `stockwise backtest portfolio TCS INFY WIPRO -s rsi` |
| Walk-forward test | `stockwise backtest walkforward TCS -s sma --train 1y --test 3mo` |
| Paper-trade one session, then exit (for cron) | `stockwise daemon intraday 25000 --once` |
| Replay the 2020 crash | `stockwise sim replay start covid-crash-2020` |
| Take a challenge | `stockwise sim challenge start beat-the-index` |
| Your badges and best scores | `stockwise sim profile` |
| Shareable result card | `stockwise sim card --svg card.svg` |

## AI assistants

| Do this | Command |
|---|---|
| Ask in plain English (local Ollama by default) | `stockwise ask "how did my portfolio do this month?"` |
| See which tools it ran | `stockwise ask "compare TCS and INFY on RSI and risk" --verbose` |
| MCP server for Claude, Cursor and other assistants | `stockwise mcp` |
| Add it to Claude Code | `claude mcp add stockwise -- stockwise mcp` |
| The MCP tools and their schemas | `stockwise mcp --list-tools` |
| MCP over HTTP | `stockwise mcp --http 127.0.0.1:8788` |

## Dashboards and scripting

| Do this | Command |
|---|---|
| Live terminal dashboard | `stockwise tui` |
| Web dashboard and JSON API | `stockwise serve --open` |
| Pipe into jq | `stockwise quote TCS INFY --json \| jq '.[].regularMarketPrice'` |
| Save your defaults | `stockwise config init` |
| Shell completion | `stockwise completions zsh > ~/.zfunc/_stockwise` |

## Live trading (real money)

| Do this | Command |
|---|---|
| Connect a broker | `stockwise trade setup` |
| Check a broker read-only | `stockwise trade selftest` |
| Preview an order, send nothing | `stockwise trade buy RELIANCE 1 --dry-run` |
| Live holdings | `stockwise trade holdings` |

Every order shows a preview and asks you to type `yes`. Practise with
`stockwise sim` first. StockWise is not financial advice — see the
[disclaimer](https://github.com/iamsaquib8/stockwise/blob/main/DISCLAIMER.md).
