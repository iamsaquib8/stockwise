# Methodology

How StockWise turns market data into scores and actions. Every rule here is
applied the same way to every stock, and you can check it yourself. When a
rule changes, its version number changes, so the track record can tell old
lists from new ones.

## Actions (methodology 1)

`stockwise verdict`, `deep`, a bare `stockwise SYMBOL`, `portfolio review`,
the Action columns of `portfolio`, `portfolio checkup` and `picks`, and the
MCP tools `stock_action` and `portfolio_review` all show the same action. It
comes from one module, `src/action.rs`, and nothing else decides it. The
rules are deterministic: the same data always gives the same action. When a
rule or threshold below changes, `METHODOLOGY` in `src/action.rs` goes up.

| You don't hold it | You hold it | Not enough data |
|---|---|---|
| BUY, WATCH, AVOID | ADD, HOLD, TRIM, EXIT | NO CALL |

Every action carries:

- **reasons**: up to three, most important first;
- **confidence**: high, medium or low (below);
- **invalidation price**: a daily close below it invalidates the action. It
  is set for BUY, ADD, HOLD and TRIM; WATCH, AVOID and EXIT have none;
- **review by**: the date to look again;
- **size hint** (BUY, ADD, TRIM) and **tax note** (TRIM, EXIT), when the
  data allows;
- the methodology version, the horizon and the disclaimer.

An action is a rule-based educational signal, not personalised advice. See
[DISCLAIMER.md](../DISCLAIMER.md).

### 1. When there is no call

The engine says NO CALL, with the reason or the missing fields
(`ACTION: NO CALL — missing: ROE, D/E`), when:

- the instrument is not a listed stock: funds, ETFs, Sovereign Gold Bonds,
  indices, FX and crypto get NO CALL;
- there is no price;
- **data coverage is below 60%**. It never guesses;
- there is no composite score.

Coverage is the share of key fields the data sources supplied (the
`coverage` figure in each command's data footer, 0 to 100).

### 2. The composite score

- `verdict`, `deep` and a bare `stockwise SYMBOL` use the **6-pillar
  composite** of `verdict`. Safety is its Balance Sheet pillar; valuation is
  its Valuation pillar.
- `portfolio review`, the portfolio Action columns and `picks` work from a
  quote, not a full research fetch. They use the **quote-based long-term
  score** (`longterm`) as the composite, with its safety and valuation
  pillars.

A pillar of 65 or more is **Strong**; below 45 is **Weak**.

### 3. Hard gates

Any one of these makes a stock you don't hold **AVOID** and one you hold
**EXIT**:

- Altman Z (or Z'') in the distress zone;
- negative shareholders' equity (book value below zero);
- loss-making (negative EPS or profit margin) with debt to equity above 1.5×
  (not for banks and other financials);
- promoter pledge above 25% of the promoter holding;
- composite below 45 (stocks you don't hold only).

### 4. Small-cap gates

A small cap has a market cap below **₹5,000 crore** (Indian rupee quotes) or
**$2 billion** (US-dollar equivalent, other currencies). A stock with no market
cap is not treated as a small cap. On both horizons, any one of these makes a
small cap AVOID, or EXIT if you hold it:

- promoter pledge above 10%;
- average value traded per session over the last 20 sessions (close × volume)
  below the `picks` floor for its tier: ₹1 crore a day for a small cap and
  ₹50 lakh for a micro cap in India, $2 million and $0.5 million in the US
  (other markets have no floor);
- more than 2 price-band (circuit) hits in the last 20 sessions, NSE/BSE only
  (see the heuristic below);
- an auditor resignation, or a qualified, adverse or disclaimer of opinion, in
  the filing titles;
- exchange surveillance (an NSE ASM/GSM stage), a trading suspension or a
  delisting among the data warnings.

**Circuit heuristic.** Daily bars don't say whether a stock hit its price
band. A session counts as a hit when the close moved 5% or more from the
previous close and closed at the day's high or low (within 0.1%). Only NSE
and BSE listings are counted.

A small cap's size hint is capped at half of `max_stock_weight`.

### 5. A stock you don't hold: BUY, WATCH, AVOID

After the gates, the stock is **BUY** when all of these hold:

- composite 65 or more;
- the safety pillar is not Weak (a missing safety pillar does not block);
- the last close is above the 200-day average;
- a positive P/E no more than **1.2×** a reference P/E. The reference is the
  stock's own median P/E or the sector P/E, whichever is higher. The own
  median is the trailing P/E (four quarters of EPS over that day's close) at
  each quarter end inside the price history available, up to 5 years; it
  needs at least three quarter ends. The sector P/E is the NSE sector index
  P/E or the median of the stock's peers. The quote-based views have no
  quarter history, so they use the sector P/E only. A missing P/E or
  reference fails this rule;
- the invalidation price is below the last close.

Short horizon adds two more: the close is above the **50-day average**, and
the 20-day average volume is at least **1.2×** the 60-day average.

If any rule fails, the action is **WATCH**, and the failed rules come first
in the reasons. If every rule holds but results are due within **5 days**,
the action is WATCH until they are out.

### 6. A stock you hold: EXIT, TRIM, ADD, HOLD

The rules run in this order; the first that fires decides.

**EXIT** when any of these hold:

- a hard gate or small-cap gate (above);
- composite below 40;
- a limit in your note for the stock is broken (`notes.json`: maximum P/E,
  minimum ROE, maximum weight);
- the last close is below the invalidation price.

**TRIM** when any of these hold:

- the holding's weight is above `max_stock_weight` (half of it for a small
  cap);
- the valuation is stretched **and** the gain on the holding is above 50%.
  Stretched means a P/E above **1.5×** the reference P/E (as for BUY), or,
  when there is no reference, a valuation pillar below 35;
- counting what your funds and ETFs hold of the same company (`xray`), the
  company is above the cap while the direct holding is not.

**ADD** when all of these hold:

- composite 65 or more;
- the trend is up. Long horizon: close above the 200-day average and the
  50-day average above the 200-day. Short horizon: close above the 50-day
  average and the 20/60-day volume ratio at least 1.2;
- the weight is known and below the cap (and below your `portfolio target`
  for it, if you set one).

If results are due within 5 days, ADD becomes **HOLD** until they are out.

**HOLD** otherwise.

### 7. Invalidation price

- **Long horizon:** the higher of the 200-day average and
  reference − 2.5 × ATR(14).
- **Short horizon:** the higher of reference − 1.5 × ATR(14) and the lowest
  low of the 20 sessions before the last one.

If one input is missing, the other is used. ATR(14) is the 14-session average
true range. The reference is the last close for a stock you don't hold. For a
holding it is the **highest close of the last 20 sessions**, so the level
trails the price up instead of moving with each day's close. A holding that
closes below its invalidation price is an EXIT.

### 8. Review date

- **Long horizon:** the next results date if it falls within 90 days, else
  90 days from today.
- **Short horizon:** 10 trading sessions from today (weekends skipped,
  exchange holidays not).

### 9. Size hint

For BUY and ADD, the largest position the rules allow is

    risk_per_trade ÷ ((price − invalidation) ÷ price)

as a percent of the portfolio, so a close at the invalidation price would cost
about `risk_per_trade` percent of the portfolio. It is capped by
`max_stock_weight`, or half of it for a small cap. `risk_per_trade` and
`max_stock_weight` come from your investor profile. When the portfolio value
is known, the hint also gives the number of shares to buy to reach that
weight (less any you hold). The hint names the limit that set it:
`risk_per_trade`, `max_stock_weight` or `small_cap_half_cap`.

For TRIM, the hint is the cap. When the holding is above it, the hint gives
the shares to sell to get back under it: shares × (1 − cap ÷ weight), rounded
up.

### 10. Tax note

For TRIM and EXIT (except an EXIT forced by a hard or small-cap gate): when a
lot turns long-term within 30 days and waiting would save tax, the note says
so: "a lot turns long-term on 14 Dec 2026 — waiting 23 days saves about
₹4,120 in tax".

### 11. Confidence

- **Low:** coverage below 70%, or the pillars are mixed (at least two Strong
  and at least two Weak).
- **High:** coverage 85% or more and the pillars agree with the action. For
  BUY and ADD, no pillar is Weak. For AVOID, EXIT and TRIM, no pillar is
  Strong or Weak pillars outnumber Strong ones. For WATCH and HOLD, the
  pillars are not split between Strong and Weak. An action forced by a hard
  or small-cap gate always agrees.
- **Medium:** otherwise.

NO CALL is always low confidence.

### 12. Data limits

- **Pledge.** The data sources often give only whether promoter shares are
  pledged, not the percentage. Without the percentage, the pledge gates
  (25%, and 10% for small caps) cannot fire.
- **Results dates.** A plain quote carries no results date. `portfolio
  review` and the Action columns therefore review in 90 days on the long
  horizon, and their 5-day results rule cannot fire.
- **Auditor flags** come from filing titles, which only the single-stock views
  (`verdict`, `deep`, bare `stockwise SYMBOL`) fetch.
- **Composite.** `portfolio review` uses the quote-based long-term score; the
  single-stock views use the 6-pillar composite. The two can differ for the
  same stock.

**Honest limit.** Fundamentals have no point-in-time history: the P/E, ROE or
debt a stock had on a past date isn't available, only today's. So the actions
can't be properly backtested without look-ahead bias, and no backtest is
claimed.

## Picks (methodology version 2)

`stockwise picks` ranks the stocks of a market against each other and lists
the highest scores. Version 2 replaced the 0.8 method, which put stocks with
broken or one-off figures at the top of the list.

### 1. Who gets scored

The scan starts from the market's universe. For India, that is the stocks
listed on BSE that traded at least ₹10 lakh on the last session, plus NIFTY
500. This is a prerequisite of the universe, separate from the floors below:
`--min-adv 0` does not bring in a BSE-only stock under ₹10 lakh (use
`--universe all` for every BSE stock). For the US, the universe is the
S&P 1500. Only stocks that clear two floors are scored:

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

With a positive `--top`, at most **3 stocks per sector** are listed. Only
stocks with a known sector count toward the limit. `--top 0` lists every
stock without it.

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
  with the last year's price return (held to −10%…25%). When only one of
  the two is known, that one is used. Only when both are missing does the
  score's band stand in, and the reasons say so. The final figure is held
  to 0%…30%: for example, −5% and −10% blend to −6.5% and are reported as
  0%.
- The Monte Carlo range uses log returns whose median path compounds at the
  assumed return. Its median matches the 5-year projection; volatility only
  widens the range.

The bulk quote has no margins, debt, beta or growth, so the long-term
finalists get their fundamentals fetched before the final score.

## Indicators

- **RSI** of a flat series is 50, not 100 (nothing rose, so nothing is
  overbought).
- **MACD** needs 34 bars for its signal line. With fewer it gives no reading,
  instead of a signal line of zero.
