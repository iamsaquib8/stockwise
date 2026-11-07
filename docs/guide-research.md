# Check a stock or fund fast

This guide shows how to size up a stock or a mutual fund in a few minutes: a
quote, a one-page overview, accounting-quality scores, a timeline of events and
what a fund's past SIP returned.

You need nothing but StockWise. No account or API key is required. Data comes
from Yahoo Finance, NSE and AMFI, so quotes can be delayed.

None of these commands tells you what to buy or sell. They show facts and
simple scores. The decision stays with you.

## 1. Find the ticker

```bash
stockwise search "tata motors"
```

Bare tickers use the Indian market, so `TCS` means `TCS.NS`. For other markets,
add `-m us`, `-m crypto`, `-m fx`, `-m commodity` or an exchange such as
`-m uk`. See [Markets](markets.md).

## 2. Get the price

```bash
stockwise quote RELIANCE TCS     # one block per stock
stockwise quote AAPL NVDA -m us
stockwise TCS                    # no command: a one-screen overview
```

```text
  RELIANCE.NS — Reliance Industries Limited
  ₹1320.00  ▲ 6.60 (0.50%)

  Open                         ₹1295.00
  Day Range                    ₹1295.00 — ₹1320.00
  52-Week Range                ₹1152.20 — ₹1611.80
  Volume                       22.77M
  Avg Volume (3M)              14.49M
  Market Cap                   ₹17.86L Cr
```

Short forms exist for the common commands: `q` for `quote`, `a` for `analyze`,
`t` for `technical` and `c` for `compare`.

## 3. Read the full picture for one stock

```bash
stockwise deep HDFCBANK
```

`deep` puts company details, valuation, profitability, financial health,
dividends, analyst ratings, a one-year chart, technical indicators, support and
resistance, risk and the six-pillar score on one screen. It is long. Scroll to
the part you need.

For smaller pieces, use these:

| Question | Command |
|---|---|
| Are the fundamentals sound? | `stockwise analyze INFY` |
| How volatile is it, and how deep were its falls? | `stockwise risk HDFCBANK` |
| Where is it against its moving averages and RSI? | `stockwise technical TCS -p 6mo --explain` |
| How does it compare with its peers? | `stockwise compare TCS INFY WIPRO` |
| What did it return over 1, 3 and 5 years? | `stockwise returns TCS` |

`--explain` adds plain-English notes to `technical`, `score`, `verdict` and
`options oi`.

## 4. Check accounting quality

```bash
stockwise score INFY --explain
```

```text
  Piotroski F-Score:   6/9  ███████░░░  Moderate / Stable
  Altman Z-Score:      8.31  Safe Zone (Low Default Risk)
```

The Piotroski F-score counts nine yes-or-no tests of profitability, leverage
and efficiency. The Altman Z-score estimates distress risk. `--explain` shows
each test, its formula and the value used. When data is missing, StockWise says
how many tests it could run instead of guessing.

## 5. See what happened and what the data says

```bash
stockwise events TCS      # filings, results, news and analyst moves, with the price reaction
stockwise insights TCS    # positive and negative findings on ownership, growth, margins, valuation
```

`events` lists the last 90 days on a timeline. Each event shows how the stock
moved against the index afterwards. `insights` groups findings such as promoter
pledge, institutional buying and selling, and margin trends. Both are summaries
of public data, not forecasts.

Upcoming results and dividend dates for your holdings:

```bash
stockwise calendar --holdings
```

## 6. Check a mutual fund

```bash
stockwise mf search parag flexi direct   # find the AMFI code
stockwise mf nav 122639                  # latest NAV and expense ratio
stockwise mf sip 122639 5000 -p 5y       # what ₹5,000 a month became
```

```text
     Code  Scheme                                                                 NAV        Date
   122639  Parag Parikh Flexi Cap Fund - Direct Plan - Growth                 93.9483 06 Nov 2026
```

`mf sip` replays a monthly SIP over the past five years and prints the amount
invested, the final value and the XIRR. It describes the past. Compare funds in
one category with `stockwise mf screen flexi cap`.

## 7. Options and IPOs (optional)

```bash
stockwise options chain NIFTY     # strikes with open interest, IV and greeks
stockwise options oi NIFTY        # open-interest walls, put/call ratio, max pain
stockwise ipo                     # open and upcoming IPOs from NSE
```

Add `--explain` to `options oi` for plain-English notes.

## Next steps

- Want a message when a price or RSI level is hit? See
  [Set up alerts and the daemon](guide-alerts.md).
- Want to test a rule on past prices? See
  [Backtest and practise on past markets](guide-practice.md).
- Want an assistant to run these lookups for you? See
  [Use StockWise from Claude or Cursor](guide-mcp.md).
- Every option for these commands is in the commands reference:
  [Analysis](commands.md#analysis), [Market](commands.md#market) and
  [Funds and F&O](commands.md#funds--fo).
- Quote or data errors: [Troubleshooting](troubleshooting.md).
