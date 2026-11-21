# Review your holdings with rule-based actions

This guide answers "what do the rules say about my holdings?". It shows how to
get an action for every holding and for one stock, how to read each part of
it, and what to check before you decide.

You need: a portfolio. If you have not built one yet, start with
[Import your trades](guide-portfolio.md). For one stock you need nothing.

> Actions are rule-based educational signals from StockWise's published
> [methodology](methodology.md#actions-methodology-1). They are not
> personalised advice. StockWise is not a SEBI-registered investment adviser
> or research analyst, and you can lose money. See the
> [disclaimer](disclaimer.md).

## 1. What an action is

The same rules run for every stock and always give the same answer for the
same data. The label depends on whether you hold the stock:

| You don't hold it | You hold it | Not enough data |
|---|---|---|
| BUY, WATCH, AVOID | ADD, HOLD, TRIM, EXIT | NO CALL |

Every action carries:

- **reasons**: up to three, most important first;
- **confidence**: high, medium or low, from data coverage and how far the
  pillars agree;
- **invalidation price** (BUY, ADD, HOLD, TRIM): a daily close below it
  invalidates the action;
- **review date**: when to look again;
- a **size hint** (BUY, ADD, TRIM) and a **tax note** (TRIM, EXIT) when the
  data allows.

## 2. Review every holding

```bash
stockwise portfolio review
```

Output from a sample portfolio of nine large caps on 21 Nov 2026 (trimmed):

```text
  Portfolio review (long horizon)
  2 exit · 0 trim · 2 add · 5 hold · 0 no call   (long horizon, methodology 1)

  INFY.NS  INFOSYS LIMITED  11.2% of portfolio  P&L -26.8%
    ACTION: EXIT · medium confidence · review by 13 Jan 2027
      • Closed at ₹1,098.00 below the invalidation level ₹1,179.91

  ICICIBANK.NS  ICICI BANK LTD.  11.0% of portfolio  P&L +31.2%
    ACTION: ADD · high confidence · invalid below ₹1,562.36 · review by 14 Jan 2027
      • Composite score 77 (65 or more)
      • Trend up: price above a rising average
      • Weight 11.0% leaves room under the 12.0% cap
      • Size: at most 12.0% of the portfolio, about 3 shares (max_stock_weight)

  HDFCBANK.NS  HDFC BANK LTD  11.0% of portfolio  P&L +5.8%
    ACTION: HOLD · high confidence · invalid below ₹801.66 · review by 16 Jan 2027
      • Composite score 78: no exit, trim or add rule triggered
      • Trend not up (price and 50-day average not both above the 200-day), so no add
      • Price ₹846.00 is above the invalidation level ₹801.66

  Rule-based educational signal from StockWise's published methodology, not investment advice. ...
```

Holdings come sorted EXIT, TRIM, ADD, HOLD, then NO CALL, so the ones the rules
flag come first. The same action is in the Action column of
`stockwise portfolio` and `stockwise portfolio checkup`.

Options:

```bash
stockwise portfolio review --horizon short   # weeks to months: tighter invalidation, 10-session review
stockwise portfolio review --json            # the same, for scripts
stockwise --portfolio long-term portfolio review
```

## 3. Get the action for one stock

```bash
stockwise verdict SUNPHARMA                  # action, then the 6-pillar score behind it
stockwise verdict SUNPHARMA --horizon short
stockwise SUNPHARMA                          # one-screen overview, starts with the action
stockwise deep SUNPHARMA                     # everything, starts with the action
```

```text
  ACTION: WATCH · low confidence · review by 28 Jan 2027
    • Composite score 59 is below 65
    • No P/E history or sector P/E to check valuation against
    • Price above its 200-day average (₹1,821.49)
    Methodology 1 (long horizon). Rule-based educational signal from StockWise's published methodology, ...
```

A stock you hold gets ADD, HOLD, TRIM or EXIT here too. `verdict --explain`
lists the pillar weights and rules.

## 4. Read each part

**Reasons.** They say which rule decided. For WATCH, the failed BUY rules come
first. For EXIT, the first reason names the gate or the broken level. Check
each reason against the data yourself: `stockwise analyze SYMBOL`,
`stockwise technical SYMBOL`, `stockwise events SYMBOL`.

**Confidence.** Low means coverage is below 70% or the pillars are mixed.
High means coverage is 85% or more and the pillars agree with the action.
Treat a low-confidence action as a prompt to look closer, not as an answer.

**Invalidation price.** On the long horizon it is the higher of the 200-day
average and the reference price minus 2.5 × ATR(14); on the short horizon,
the higher of the reference minus 1.5 × ATR(14) and the 20-session low. For a
holding the reference is the highest close of the last 20 sessions, so the
level trails the price up. A holding that closes below it becomes an EXIT.

**Review date.** Long horizon: the next results date within 90 days, else 90
days out. Short horizon: 10 trading sessions out. Look again by then, or
sooner if the price closes below the invalidation level.

**Size hint.** For BUY and ADD, the largest weight your profile allows
(`risk_per_trade`, capped by `max_stock_weight`, half of it for a small cap).
For TRIM, the shares to sell to get back under the cap. Set your own limits
with `stockwise profile set active:low` (or another mode); see
`stockwise profile show`.

**Tax note.** For TRIM and EXIT: when a lot turns long-term within 30 days and
waiting saves tax, the note says how many days and about how much.

## 5. When the answer is NO CALL

```text
  ACTION: NO CALL — missing: market cap, P/B, ROE, D/E, profit margin, revenue growth, beta, sector, shareholding
    • Data coverage 25% is below the 60% needed for a call
```

The rules never guess. Funds, ETFs, Sovereign Gold Bonds, indices, FX and
crypto always get NO CALL, and so does a stock with less than 60% of its key
data. The `missing:` list names what did not come through. Try again later,
or set a keyed data provider ([data providers](data-providers.md)).

## 6. Before you act

An action tells you which rule fired. It does not know your full finances,
goals or tax position. Before you place an order:

1. Read the reasons and check the figures behind them.
2. Preview a sale and its tax: `stockwise portfolio sell INFY 10 --preview`.
3. For TRIM or EXIT, read the tax note, and see
   [File your Indian capital-gains tax](guide-tax.md).
4. To raise cash across holdings at the lowest tax, use
   `stockwise rebalance --raise 50000` ([Rebalance](guide-rebalance.md#5-raise-cash-at-the-lowest-tax)).

StockWise never places an order from an action. If you trade, record it so
the next review starts from the right place: record it with `portfolio add`
or `portfolio sell`, or import the new tradebook.

## 7. Get told when it changes

Put an alert on the invalidation price, checked on the daily close:

```bash
stockwise alert add HDFCBANK below 801.66 --on-close
```

The background worker also watches every holding and sends one digest after
the close. See [See what's hot and what changed today](guide-daily.md).

## Next steps

- Look for stocks you don't hold yet:
  [Find candidates with picks](guide-picks.md).
- Ask an assistant to explain the actions: the MCP tools `stock_action` and
  `portfolio_review` return the computed action, and the assistant explains
  it. See [Use StockWise from Claude, Gemini, Codex or Cursor](guide-mcp.md).
- Every rule and threshold: [Methodology: Actions](methodology.md#actions-methodology-1).
- Command options: [Actions](commands.md#actions) and
  [Portfolio review](commands.md#portfolio-review).
