# Methodology

How StockWise turns market data into scores. Every rule here is applied the
same way to every stock, and you can check it yourself. When a rule changes,
its version number changes, so the track record can tell old lists from new
ones.

## Picks (methodology version 2)

`stockwise picks` ranks the stocks of a market against each other and lists
the highest scores. Version 2 replaced the 0.8 method, which put stocks with
broken or one-off figures at the top of the list.

### 1. Who gets scored

The scan starts from the market's universe: the stocks listed on BSE that
traded on the last session, plus NIFTY 500, for India; the S&P 1500 for the
US. Only stocks that clear two floors are scored:

| Market and tier | Market cap at least | Average traded value a day (3 months) at least |
|---|---|---|
| India (default, large and mid caps) | ₹500 crore | ₹2 crore |
| India, `--cap small` | ₹500 crore | ₹1 crore |
| India, `--cap micro` | ₹100 crore | ₹50 lakh |
| US (default) | $1 billion | $5 million |
| US, `--cap small` | $300 million | $2 million |
| US, `--cap micro` | $50 million | $0.5 million |

A stock without a market cap or a trading history doesn't clear a floor.
`--min-mcap` and `--min-adv` change the floors (in crore in India, millions
elsewhere); 0 turns one off. The liquidity floor matters most for small
caps: a stock that hardly trades is easy to move, and hard to sell when it
falls.

### 2. Two stages

The bulk quote has prices, averages, the 52-week range, EPS, book value and
dividends, but no return on equity, margins, debt, revenue growth, beta or
analyst count. So the scan runs twice:

1. Every stock is ranked on what the bulk quote has.
2. The leaders of that first pass get their fundamentals and a year of daily
   prices and volumes. That is four times `--top`, at least 60 and at most
   200. Then the whole scan is ranked again.

### 3. Measures and pillars

Each measure is a **percentile rank** among the stocks scanned: 100 means
highest in this scan, 50 means the middle. A rank describes a stock's place
in the set, not a threshold it crossed. For costs, lower ranks higher:
volatility, drawdown, beta and debt.

| Pillar | Measures (weight within the pillar) |
|---|---|
| Value | earnings yield (50%), book value to price (30%), dividend yield (20%) |
| Quality | return on equity (50%), profit margin (30%), low debt to equity (20%) |
| Growth | forward against trailing EPS, only with 3+ analysts (60%); revenue growth (40%) |
| Trend, long term | 12-1 month momentum divided by volatility (60%), price against the 200-day average (40%); without a price history, the 200-day average (60%) and the place in the 52-week range (40%) |
| Trend, short term | 3-month momentum to a week ago, divided by volatility (35%); price against the 50-day average (20%); place in the 52-week range (20%); last 20 sessions' volume against the 60 before (25%) |
| Risk | low volatility (40%), small drawdown over the year (30%), low beta (30%) |

Pillar weights depend on `--horizon`:

| Pillar | `long` (default: years) | `short` (weeks to a few months) |
|---|---|---|
| Value | 25% | 10% |
| Quality | 25% | 15% |
| Growth | 15% | 10% |
| Trend | 20% | 45% |
| Risk | 15% | 20% |

The short-term list still gives quality and value 25% between them. Momentum
in a weak business is how most short-term losses start.

### 4. Distress is scored, not skipped

- **Negative book value** (or negative debt to equity, which means the same):
  quality is 0, and book value to price ranks last.
- **A trailing loss**: earnings growth ranks 0 (there is no growth rate from a
  loss), and so does the margin when it is missing. Earnings yield is
  negative, so it ranks below every profit.

In 0.8 these cases counted as "missing data" and drifted to the middle of the
list.

### 5. Missing data

Inside a pillar, a missing measure leaves its weight to the others. Coverage
counts this: a pillar with half its measures counts as half. The score is
then pulled toward 50 in proportion to the missing share:

```
score = 50 + (weighted pillars − 50) × coverage
```

A stock with less than **60%** coverage is left out of the list; the scan
says how many were. A pick that rests on a sliver of data isn't a pick.

### 6. Figures too good to be true

These are flagged **check data** and left out of the list:

- P/E below 3
- return on equity above 80%
- dividend yield above 15%

Such figures are far more often a one-off gain, a stale row or a reporting
error than a bargain. In 0.8, capped figures like these tied for first place
on value and quality. The list names the flagged stocks so you can look them
up.

The dividend yield is the corrected one (`Quote::dividend_yield_fraction`).
Yahoo's trailing yield is 0 for many NSE stocks and wrong by a factor of 80
for Infosys.

### 7. Diversity

At most **3 stocks per sector** in a list (`--top 0` lists every stock
without the limit).

### 8. Track record

Every `picks` run appends its list to `signal_log.json` in the data folder:
date, market, scope, horizon, rank, score, price and methodology version. A
re-run on the same day doesn't log a stock twice. `cache clear` never
touches this file.

`stockwise picks review [--since 90d] [--horizon long|short]` measures each
logged pick from the day it was first listed:

- the return since then (price only; dividends not included)
- the benchmark's return over the same days: NIFTY 50 (price index) for
  India, the S&P 500 total return for the US
- the excess return, the share of picks that rose, the share that beat the
  benchmark, and mean and median figures.

**Honest limit.** Fundamentals have no point-in-time history. The ROE or
margin a stock had on a past date isn't available, only today's. So this
method can't be backtested without look-ahead bias, and no backtest is
claimed. The forward track record, which builds up from the first run, is
the evidence. Check it before trusting a list.

## Long-term scores (`longterm`)

- The composite leaves out pillars without data instead of counting them at
  50.
- A debt-to-equity ratio above 250 is its own, worse band.
- A negative P/E is a loss and lowers the valuation pillar.
- The **assumed annual return** comes from the earnings yield plus growth
  (revenue growth, else earnings growth, held to −10%…15%), blended 70/30
  with the last year's price return (held to −10%…25%). It is never read off
  the score. Without either input, the score's band stands in, and the
  reasons say so.
- The Monte Carlo range uses log returns whose median path compounds at the
  assumed return. Its median matches the 5-year projection; volatility only
  widens the range.

The bulk quote has no margins, debt, beta or growth, so the long-term
finalists get their fundamentals fetched before the final score.

## Indicators

- **RSI** of a flat series is 50, not 100 (nothing rose, so nothing is
  overbought).
- **MACD** needs 35 bars for its signal line. With fewer it gives no reading,
  instead of a signal line of zero.
