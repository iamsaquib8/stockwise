# Cheatsheet

One page of the commands people use most. `stockwise <command> --help` shows
every option; the [commands reference](commands.md) explains each one.

Bare tickers use the Indian market (`TCS` → `TCS.NS`). Add `-m us`, `-m uk`,
`-m jp`, `-m crypto`, `-m fx`, `-m commodity` … for other markets, or set
`STOCKWISE_MARKET`. Almost every data command takes `--json` (and tables take
`--csv`).

## Quotes and markets

| Do this | Command |
|---|---|
| Quote Indian stocks | `stockwise quote RELIANCE TCS` |
| Quote US stocks | `stockwise quote AAPL NVDA -m us` |
| Everything about one stock | `stockwise deep HDFCBANK` |
| Indian and US indices | `stockwise markets` |
| One index per exchange worldwide | `stockwise markets --global` |
| Gainers and losers | `stockwise movers` |
| Sector performance | `stockwise sectors` |
| Find a ticker by name | `stockwise search "tata motors"` |
| Check your setup | `stockwise doctor` |

## Analysis

| Do this | Command |
|---|---|
| Fundamentals in plain English | `stockwise analyze INFY` |
| RSI, MACD, Bollinger Bands | `stockwise technical TCS -p 6mo` |
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
| Import from your broker | `stockwise import kite holdings.csv` (also `groww`, `upstox`, `indmoney`, `cas`) |
| Compare with NIFTY | `stockwise portfolio benchmark NIFTY -p 1y` |
| Splits, bonuses, dividends | `stockwise portfolio actions --apply` |
| Capital-gains estimate | `stockwise tax in` |
| ITR Schedule CG CSV | `stockwise tax export in --out cg.csv` |
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
| What a past SIP became | `stockwise sip NIFTYBEES 5000 -p 5y` |
| What a lump sum became | `stockwise whatif TCS 100000 2015-01-01` |

## Strategies and practice

| Do this | Command |
|---|---|
| Backtest a strategy (after costs) | `stockwise backtest INFY -s macd` |
| Backtest a basket | `stockwise backtest portfolio TCS INFY WIPRO -s rsi` |
| Walk-forward test | `stockwise backtest walkforward TCS -s sma --train 1y --test 3mo` |
| Replay the 2020 crash | `stockwise sim replay start covid-crash-2020` |
| Take a challenge | `stockwise sim challenge start beat-the-index` |
| Your badges and best scores | `stockwise sim profile` |
| Shareable result card | `stockwise sim card --svg card.svg` |

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
[disclaimer](https://github.com/iamsaquib8/stockwise/blob/master/DISCLAIMER.md).
