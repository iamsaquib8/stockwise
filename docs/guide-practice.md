# Backtest and practise on past markets

This guide covers two ways to learn from history without risking money. A
**backtest** replays a rule on past prices. A **replay** lets you trade a past
market one day at a time, with the future hidden. Both use paper money and
neither touches your portfolio or a broker.

You need: StockWise installed and a network connection for price history.

Both describe the past. They are not predictions and not financial advice.

## Part 1: Backtest a rule

### Run one

```bash
stockwise backtest INFY -s macd
```

```text
  Backtest: INFY.NS — macd (2y, 2024-11-06 → 2026-11-06)
  Entry                        macd_hist(12, 26, 9) crosses_above 0
  Exit                         macd_hist(12, 26, 9) crosses_below 0
  Fills                        next bar's open · capital ₹100000.00
  Costs                        India delivery — brokerage ₹20 or 0.1%, lower, STT, ...

  Net Return (after costs)     -16.60%
  Gross Return (no costs)      -10.96%
  Costs Paid                   ₹5347.83 (5.35% of capital)
  Buy & Hold Return            -35.22%
  Alpha (net vs B&H)           +18.62%
  Max Drawdown                 -33.72%
  Trades                       16 (31% winners)
```

The numbers are an example. They change with the date you run it.

How to read it:

- **Net return** is after costs: brokerage, STT, exchange charges, GST, stamp
  duty and slippage. **Gross return** is before them. A rule that trades often
  can lose most of its gain to costs.
- **Buy and hold** is the benchmark. Compare the net return with it. A rule that
  loses less than buy and hold in a falling market has not made money.
- **Max drawdown** is the deepest fall from a peak. Ask whether you would have
  held on through it.
- Entries and exits are decided on the close and filled at the **next open**, as
  you could really trade.

### Try variations

```bash
stockwise backtest strategies                       # list the built-in strategies
stockwise backtest TCS -s rsi -P oversold=25        # change a parameter
stockwise backtest AAPL -m us -s sma -p 5y          # US stock, five years
stockwise backtest portfolio TCS INFY HDFCBANK -s sma --rebalance monthly
stockwise backtest walkforward RELIANCE -s rsi --train 1y --test 3mo
```

A walk-forward test picks the best parameters on one year of data, trades the
next three months with them, then moves forward. Only those later windows
count. This guards against fitting a rule to old data. Try it before you trust
a result that looks good.

To test your own rules, write a small TOML file and run
`stockwise backtest TCS -s file:my_rules.toml`. The format is in
[Backtesting](backtesting.md#strategy-files).

Two simpler questions have their own commands:

```bash
stockwise sip NIFTYBEES 5000 -p 5y          # what a monthly SIP became
stockwise whatif TCS 100000 2015-01-01      # what a lump sum became
```

## Part 2: Practise on a past market

### Start a replay

```bash
stockwise sim replay list                    # the built-in scenarios
stockwise sim replay start covid-crash-2020  # 3 Feb 2020, ₹1,00,000 of paper money
stockwise sim challenge start crash-survivor # optional: play for a goal
```

You see the account and every stock's close, its change on the day and over 20
trading days. That is all you know, as on the real morning. Scenarios include
the 2020 crash, the 2008 crisis, the 2023 Adani report, the 2000 dot-com bust
and the 2021 bull run.

### Trade one day at a time

```bash
stockwise sim replay chart TCS --days 120        # price history up to today's close
stockwise sim replay buy TCS 10 --stop 1900      # set where you are wrong first
stockwise sim replay next                        # orders fill at tomorrow's open
stockwise sim replay status                      # positions, orders and the market
stockwise sim replay next 5                      # five trading days at once
stockwise sim replay sell TCS                    # the whole position, at the next open
```

Orders fill at the next day's open. Stops and targets are checked against each
day's range. Costs are charged as in a real India delivery trade.

### Finish and read the score

```bash
stockwise sim replay end            # scorecard, badges and what happened
stockwise sim profile               # your badges and best scores
stockwise sim card --svg card.svg   # a shareable result card
```

The score rates **how** you traded: return for the risk you took, how deep your
drawdown went, whether you kept to your own rules and how widely you spread your
money. Raw return does not score on its own.

## Next steps

- [Set up alerts and the daemon](guide-alerts.md). The daemon is a paper-trading
  bot that runs sessions for you.
- [Check a stock or fund fast](guide-research.md) before you backtest a stock.
- Script a sweep over strategies: see
  [Script it with JSON, CSV and jq](guide-scripting.md).
- Every backtest option, the cost model and the JSON shape are in
  [Backtesting](backtesting.md). Replays, challenges, scoring and your own
  scenarios are in [Practice mode](simulator.md).
- The commands are listed under
  [Strategies and bots](commands.md#strategies--bots).
