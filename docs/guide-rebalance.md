# Rebalance to target weights

This guide sets the weight you want for each holding and shows how many shares
to buy to move toward it. The most useful form puts **new money only into
holdings that are below target**, so you do not have to sell anything and book
a gain.

StockWise only does the arithmetic. It does not place orders, and the output is
not a recommendation. You choose the targets.

You need: a portfolio. See [Import your trades](guide-portfolio.md).

## 1. Set your targets

```bash
stockwise portfolio target set TCS 30
stockwise portfolio target set INFY 30
stockwise portfolio target set SBIN 40
stockwise portfolio target show
```

```text
  Target Key      Weight%
  ─────────────────────────
  INFY.NS           30.0%
  SBIN.NS           40.0%
  TCS.NS            30.0%
  ─────────────────────────
  Total            100.0%

  Drift Tolerance: Not set (default: 0.0%)
```

Each `set` prints the running total. If the targets add up to less than 100%,
the rest is shared among holdings with no target. A key can be a symbol or a
sector. Remove one with `stockwise portfolio target remove TCS`.

With no targets saved, StockWise rebalances to equal weights.

## 2. Add new money without selling

```bash
stockwise rebalance --inflow 50000
```

```text
  Portfolio Rebalance (Inflow: ₹50000.00)
  Current Value                ₹83637.00
  Projected Value              ₹133637.00

  Symbol          Current    Now% Target% Projected%   Allocation           Action
  TCS.NS        ₹14991.00   17.9%   30.0%      30.0%    ₹25100.10    BUY 10 shares
  INFY.NS       ₹17721.00   21.2%   30.0%      30.0%    ₹22370.10    BUY 18 shares
  SBIN.NS       ₹50925.00   60.9%   40.0%      40.0%     ₹2529.80     BUY 2 shares
```

Read it left to right:

- **Now%** is the current weight. **Target%** is the weight you set.
- **Projected%** is the weight after the new money goes in.
- **Allocation** is the part of the ₹50,000 that holding receives.
- **Action** is the whole-share count that fits that amount.

The new money goes to holdings below target. A holding above target, such as
SBIN here, gets little or none. Nothing is sold, so there is no capital gain to
report.

## 3. Rebalance by selling too

Without `--inflow`, StockWise also lists holdings above target as sales.

```bash
stockwise rebalance
stockwise rebalance --drift 2.0      # ignore holdings within 2 percentage points of target
```

```text
  Symbol          Current     Now%  Target%           Action
  TCS.NS        ₹14991.00    17.9%    30.0%     BUY 5 shares
  INFY.NS       ₹17721.00    21.2%    30.0%     BUY 7 shares
  SBIN.NS       ₹50925.00    60.9%    40.0%   SELL 17 shares
```

A sale books a gain or loss. Run `stockwise harvest` or `stockwise tax in`
first to see the effect. The [tax guide](guide-tax.md) explains both.

`--drift` skips small differences, so you do not trade for a 0.5% gap. Save a
default band with `stockwise portfolio target drift 2.0`.

## 4. Try a plan without saving it

Pass targets on the command line to compare options. Nothing is saved.

```bash
stockwise rebalance TCS:30 INFY:30 SBIN:40 --inflow 50000
```

## 5. Keep the plan current

After you place the orders at your broker, record them so the next run starts
from the right place:

```bash
stockwise portfolio add TCS 10 2498.50          # a purchase dated today
stockwise import kite tradebook-latest.xlsx     # or import the new tradebook
```

## Next steps

- [Import your trades](guide-portfolio.md) if holdings are missing or
  out of date.
- [File your Indian capital-gains tax](guide-tax.md) before a rebalance that
  sells.
- Use the numbers elsewhere: `stockwise rebalance --inflow 50000 --json` prints
  the same plan as JSON. See [Script it with JSON, CSV and
  jq](guide-scripting.md).
- Every option is in the commands reference under
  [Portfolio](commands.md#portfolio). A review prompt for assistants is in
  [Use StockWise from Claude or Cursor](guide-mcp.md).
