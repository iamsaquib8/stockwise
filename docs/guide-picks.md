# Find candidates with picks

This guide shows how to rank a market's stocks against each other with
`stockwise picks`, narrow the list to a horizon and a cap tier, check each
name, and see how past lists did with `stockwise picks review`.

You need nothing but StockWise. A scan reads a whole market, so it takes
from a few seconds to a minute.

> A pick is a stock that ranks high in one scan. It is not a recommendation
> to buy it. The Action column applies fixed rules and is a rule-based
> educational signal, not personalised advice. StockWise is not a
> SEBI-registered investment adviser or research analyst, and you can lose
> money. See the [disclaimer](../DISCLAIMER.md).

## 1. Run a scan

```bash
stockwise picks                       # India: BSE stocks that trade, plus NIFTY 500
stockwise picks -m us                 # the S&P 1500
stockwise picks --universe nifty100   # one NSE index
```

Each stock gets a percentile rank on five pillars (value, quality, growth,
trend, risk) among the stocks scanned. 100 is the highest in this scan, 50 the
middle. The score combines them, pulled toward 50 when data is missing.
Stocks with less than 60% of their data, and figures too good to be true
(P/E below 3, ROE above 80%, dividend yield above 15%), are left out and
counted. With a positive `--top`, at most 3 stocks per sector are listed.

## 2. Pick a horizon and a cap tier

```bash
stockwise picks --horizon short --cap small --top 8
```

- `--horizon long` (the default) is for years: value and quality carry 25%
  each, trend 20%.
- `--horizon short` is for weeks to a few months: trend carries 45%, built
  from 3-month momentum, the 50-day average, the 52-week range and rising
  volume. Quality and value still carry 25% between them.
- `--cap mega|large|mid|small|micro` keeps one tier and ranks it within
  itself. Small and micro caps get their own size and liquidity floors
  (India small: ₹500 crore market cap and ₹1 crore traded a day). Change
  them with `--min-mcap` and `--min-adv`; 0 turns a floor off.

Output on 20 Nov 2026 (the stock rows are replaced with placeholders; the
rest is as printed, trimmed):

```text
  Highest Scores, short term (weeks to months) — India (small cap)
  ⟳ 297 stocks ranked against each other on value, quality, growth, trend and risk
  ⓘ 17 more left out: market cap under ₹500 crore or under ₹1 crore traded a day (--min-mcap / --min-adv)
  ⓘ 244 more left out: less than 60% of their data came through

  Symbol              Price     Chg%  Score  Value  Quality  Growth  Trend  Risk  Data  Action       Top fifth on
  STOCK-A.NS            …        …       72     53       87      34     90    67   87%  …            Quality, Trend
  STOCK-B.BO            …        …       71     18       86      70     84    60  100%  …            Quality, Trend
  ...
  Action: rule-based (methodology 2), as if not held: BUY / WATCH / AVOID or NO CALL on thin data. The rank
  is relative to this scan; the action applies fixed rules. `stockwise verdict SYM` shows the reasons.
```

How to read it:

- **Score** and the pillar columns are ranks within this scan, not grades. A
  score of 72 means the stock ranks high among these 297, nothing more.
- **Data** is the share of the score that had data behind it.
- **Action** is the rule-based action as if you did not hold the stock. The
  rank and the action are separate: a high rank can still be WATCH or AVOID.
- **Top fifth on** names the pillars where the stock ranks 80 or more.

## 3. Check each name

A list is a starting point. For every name you look at:

```bash
stockwise verdict SBIN                # the action's reasons and the 6-pillar score
stockwise verdict SBIN --horizon short
stockwise events SBIN                 # filings, results and how the price reacted
stockwise risk SBIN                   # volatility and drawdown
```

Small caps face extra gates in the action rules: a promoter pledge above 10%,
thin trading, more than 2 price-band hits in 20 sessions, an auditor red flag
or exchange surveillance makes them AVOID. See
[Review your holdings with rule-based actions](guide-actions.md) for how to
read an action.

## 4. See how past lists did

Every `picks` run appends its list to `signal_log.json` in the data folder
(date, market, horizon, rank, score, price, methodology version). `picks
review` measures each logged pick from the day it was first listed:

```bash
stockwise picks review                         # long-horizon lists of the last 90 days
stockwise picks review --horizon short --since 6m
stockwise picks review --json
```

It prints each pick's return since it was listed, the benchmark's return over
the same days (`^NSEI` NIFTY 50 price index for India, `^SP500TR` S&P 500
total return for the US) and the excess return, then a summary line: how many
rose, how many beat the index, mean and median. Stock returns are price-only
(excluding dividends). NIFTY 50 (`^NSEI`) is also a price index excluding
dividends, whereas the US benchmark (`^SP500TR`) is a total-return index that
includes dividends — so US excess returns compare price-only stock returns
against a total-return benchmark.

Fundamentals have no point-in-time history, so the picks method cannot be
backtested without look-ahead bias. This forward track record, which starts
with your first run, is the evidence. Check it before you trust a list.

## 5. Use it in a script

```bash
stockwise picks --cap mid --top 20 --json | jq -r '.picks[] | "\(.symbol)\t\(.score)"'
stockwise picks --csv > picks.csv
```

`--json` prints an object with `picks` (each with its pillar ranks,
`coverage` and `action`), the scan counts, `horizon`, `methodology` and
`meta`, which lists the data sources, coverage and warnings.

## Next steps

- What the rules say about stocks you already hold:
  [Review your holdings with rule-based actions](guide-actions.md).
- Every measure, weight and floor: [Methodology: Picks](methodology.md#picks-methodology-version-2).
- Ask an assistant: the MCP tools `picks` and `picks_review` return the same
  lists. See [Use StockWise from Claude, Gemini, Codex or Cursor](guide-mcp.md).
- Command options: [Market](commands.md#market).
