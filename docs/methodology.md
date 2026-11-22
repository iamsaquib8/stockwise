# Methodology

How StockWise turns market data into scores and actions. Every rule here is
applied the same way to every stock, and you can check it yourself. When a
rule changes, its version number changes, so the track record can tell old
lists from new ones.

<a id="actions-methodology-1"></a>

## Actions (methodology 2)

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
  P/E or, in `picks`, the median P/E of the scan's stocks in the same sector
  (at least 3). When there is neither, `picks` falls back to the median P/E
  of every profitable stock scanned (at least 10). The quote-based views have
  no quarter history, so they have no own median. A missing P/E or reference
  fails this rule;
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
  for it, if you set one). When the look-through weight is known (what your
  funds and ETFs hold of the same company, as for TRIM), that combined weight
  must be below the cap; the target is compared with the direct weight.

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
`max_stock_weight` come from your investor profile. For ADD with a known
look-through weight, what your funds and ETFs already hold of the company is
taken off the cap first, so the hint never takes combined exposure above it. When the portfolio value
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

## IPO verdicts (methodology ipo-1)

`stockwise ipo SYMBOL`, the Verdict column of the `stockwise ipo` board and
the MCP tools `ipo_analyze` and `ipo_board` show the same verdict. It comes
from one module, `src/ipo_verdict.rs`, on the listing-day model of
`src/ipo_model.rs`; nothing else decides it. The rules are deterministic.
When a rule or threshold below changes, `METHODOLOGY` in
`src/ipo_verdict.rs` goes up.

| Verdict | Meaning |
|---|---|
| APPLY | The listing case and the long-term case both pass |
| APPLY – SELL ON LISTING | The listing case passes; valuation, the offer for sale or losses make the long-term case weak |
| WAIT | The institutional book is not final yet; the trigger says which QIB multiple by 3 PM IST on the closing day would make it APPLY |
| AVOID | A hard gate fired, or the final book gives a weak listing case |
| NO CALL | Data coverage below 60%, no calibrated model, or an SME issue without red flags |
| LISTED | The issue trades; the stock action engine ([Actions](#actions-methodology-2)) decides |

Every verdict carries up to three reasons, a confidence level (high,
medium, low), what would invalidate it (or the WAIT trigger), a review date
(the listing date, or the closing day for WAIT), the data coverage and the
fields missing, the methodology version and the disclaimer.

### 1. Data coverage

Missing data counts against coverage; nothing stands in at a neutral value.
Mainboard fields: price band, lot size, the fresh/OFS split, QIB, NII and
retail subscription (expected only once bidding opens), post-issue shares, profit after tax, P/E at the upper band,
the peer median P/E, revenue growth, promoter holding after the issue, the
OFS sellers (when there is an offer for sale) and the IPO market regime.
Below 60% the verdict is NO CALL with the missing fields listed; any gate
that already fired is shown with it.

Valuation figures come from the RHP. StockWise does not parse the RHP: you
save the figures from its "Capital Structure", "Basis for Offer Price" and
"Restated Financial Information" sections in `ipo/<SYMBOL>.toml` in the data
directory (`stockwise ipo SYMBOL --rhp-template` prints the file). The
peers named in the RHP are quoted live (Yahoo).

### 2. Valuation at the upper band

- Post-issue shares = pre-issue shares + fresh shares (fresh issue amount ÷
  upper band when the RHP count is not saved).
- Market cap = post-issue shares × upper band.
- P/E = market cap ÷ latest profit after tax.
- P/B = market cap ÷ (latest net worth + fresh issue proceeds).
- EV/Sales = (market cap + debt − cash − fresh proceeds) ÷ latest revenue.
- ROE = profit ÷ net worth; D/E = debt ÷ net worth; revenue and profit
  growth are compound annual rates over the years saved.
- OFS share = offer for sale ÷ issue size; a seller of kind `pe` or `vc`
  makes it a fund exit.
- Peer median P/E, P/B and EV/Sales from live quotes of the peers.

### 3. Hard gates (AVOID)

| Gate | Threshold |
|---|---|
| P/E against the peer median | above 1.5× |
| Offer for sale with a PE/VC fund selling | above 75% of the issue |
| Loss-making | revenue growing less than 25% a year |
| QIB subscription at close (mainboard) | below 1x |
| Total subscription at close | below 1x |
| Grey market premium | negative, read from two or more press headlines |
| SME: no QIB portion | — |
| SME: tiny float | issue under ₹10 crore |
| SME: promoter holding after the issue | under 50% |

GMP is unofficial press chatter read from headlines by pattern; it is never
a model input, only this red flag, and only when two headlines agree.

### 4. The listing-day model

```text
expected listing-day return % = b0 + b1·ln(1+QIB x) + b2·ln(1+NII x) + b3·ln(1+retail x) + b4·regime %
```

- Fitted by Huber robust regression (k = 1.345) on NSE mainboard issues:
  final subscription from NSE's issue detail, listing-day close over the
  issue price from Yahoo daily bars. SME issues are left out (they list
  under a 90% pre-open price cap and trade in lots).
- **Regime**: the recency-weighted median listing-day return of the
  mainboard issues listed in the 180 days before (weights halve every 45
  days), at least 8 of them. Fewer: the board says INSUFFICIENT DATA, the
  model uses its training mean and the gap counts against coverage. Labels:
  HOT ≥ +20%, CONSTRUCTIVE ≥ +8%, SELECTIVE ≥ 0%, COLD below 0%.
- **P(loss)**, the probability of a listing-day close below the issue
  price, is the share of the fit's residuals below −expected (21 stored
  residual quantiles), not a normal curve. The 10th–90th percentile range
  comes from the same quantiles.
- `stockwise ipo calibrate` refits the model (on NSE data, or `--csv`),
  prints n, R², residual sd and an in-sample check of P(loss), and with
  `--save` keeps the fit as `ipo_model.json`, which then takes precedence.

**Shipped fit** (2026-11-21, 192 NSE mainboard issues listed 19 Feb 2025 –
5 Nov 2026; 6 of 198 read had no Yahoo listing bar):

```text
listing % = −19.98 + 10.17·ln(1+QIB) − 6.54·ln(1+NII) + 6.73·ln(1+retail) + 0.273·regime
R² 0.42 · residual sd 20.4 points · regime training mean +8.7%
```

OLS on the same rows gives −25.53, +11.69, −5.46, +4.97, +0.487 (R² 0.43).
NII's negative weight is the overlap with QIB and retail demand (the three
move together), not a sign that NII demand hurts on its own. In-sample
check of P(loss): of the issues given under 10%, 7% closed below issue;
10–25%: 19%; 25–50%: 27%; over 50%: 68%. The fit date, n, R² and residual
sd are printed with every verdict.

### 5. Allotment odds and expected value

- **Odds** of one minimum application = lots of that size available in the
  category ÷ applications in it (NSE application counts). SEBI allots
  retail one lot, and NII the minimum sNII application (just above ₹2
  lakh), per applicant by lottery when a category is oversubscribed; a bNII
  winner (above ₹10 lakh) receives that same sNII-sized packet. Without
  application counts, 1 ÷ the share subscription is shown as a lower bound
  (every application is at least the minimum). NSE's issue detail did not
  carry application counts for the 354 issues read on 21 Nov 2026, so the
  lower bound is the usual case and the expected values are conservative.
  SME individual applications are at least two lots and above ₹2 lakh.
- **Expected value** = odds × expected listing gain × allotted amount − the
  return forgone on the blocked money (7% a year, for the days to the close
  plus 4 days to allotment and refund or listing).

### 6. The ladder

1. Listed: LISTED. SME: AVOID on a red flag, else NO CALL.
2. No model, or coverage below 60%: NO CALL.
3. A hard gate: AVOID.
4. Before 3 PM IST on the closing day: WAIT, with the trigger the smallest
   QIB multiple (at least 1x, NII and retail held at their current levels)
   at which the expected listing gain is positive, P(loss) is under 25% and
   the expected value of a retail application is positive (when the
   allotment odds are known; before bidding opens they are not). If no multiple
   up to 1,000x does it: AVOID.
5. Final book: if the expected gain is not positive, P(loss) is 25% or more,
   or the retail expected value is not positive: AVOID.
6. Otherwise APPLY when P/E is at most 1.2× the peer median, the offer for
   sale is at most 50% of the issue and the company is profitable; else
   APPLY – SELL ON LISTING.

**Confidence.** High: coverage 85%+, final book and P(loss) at most 10%.
Medium: coverage 70%+ with a regime reading. Low otherwise. AVOID on a gate
is high from 70% coverage.

### 7. After the verdict

- **Listing-day plan** (APPLY kinds): sell half the allotment at the
  model's expected gain or better (all of it for APPLY – SELL ON LISTING),
  exit the rest on a trade below the issue price, then the stock action
  engine takes over.
- **Anchor lock-in**: allotment is one working day after the close; half
  the anchor shares unlock 30 days later and the rest 90 days later, shown
  as supply overhang.
- **Track record**: every verdict is logged to `ipo_signal_log.json` (one
  per issue per day, the latest). `stockwise ipo review` takes each issue's
  last verdict before close and reports, per verdict, the hit rate
  (listing-day close above issue), mean and median listing-day return, the
  return about three months (63 sessions) after listing and the excess over
  NIFTY 50 over those months.

**Honest limits.** The model sees the book, not the business: valuation
enters only through the gates and the APPLY/SELL ON LISTING split. The fit
is in-sample; `ipo review` is the out-of-sample evidence. Exchange holidays
are not counted in the dates.

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

`--cap mega|large|mid|small|micro` keeps one tier (see the cap tiers in the
[commands reference](commands.md#market)) before scoring, so its stocks are
ranked within the tier, not against the whole market.

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

The price measures need enough daily closes: volatility and drawdown need 60,
12-1 month momentum needs 200, 3-month momentum needs 69 (a quarter plus a
week). With fewer, that measure is missing (see below).

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

Every `picks` run, and every call of the MCP `picks` tool, appends its list
to `signal_log.json` in the data folder:
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

### 9. The table's last columns

- **Top fifth on** names the pillars where the stock ranks 80 or more.
- **Action** is the rule-based action of the [Actions](#actions-methodology-2)
  rules for each finalist, as if you did not hold it: BUY, WATCH, AVOID or NO
  CALL. It uses the quote-based long-term score as the composite, a year of
  daily bars, and the scan's sector median P/E (or the scan median) as the
  valuation reference. The rank and the action are separate: the rank is
  relative to this scan; the action applies fixed rules, so a high rank can
  be WATCH or AVOID. `--horizon` sets the action's horizon too.

## Long-term scores (`longterm`)

- The composite leaves out pillars without data instead of counting them at
  50.
- A debt-to-equity ratio above 250 is its own, worse band.
- A negative P/E is a loss and lowers the valuation pillar.
  A zero P/E is treated as missing: it neither lowers nor raises the pillar.
- When both the trailing and forward P/E are above zero, the growth pillar
  has data. Only a forward P/E more than 20% below the trailing one adds
  points.
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
