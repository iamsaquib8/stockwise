# Rebalance to target weights

This guide sets the weight you want for each holding, sector or asset class and
shows the whole-share trades that move toward it. Each plan balances its cash,
says which lots a sale takes, the tax it adds and what the trades cost. The most
useful form puts **new money only into holdings that are below target**, so you
do not have to sell anything and book a gain.

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

  Drift Tolerance: ±0.0% (profile default; rebalancing ignores drifts within this band)
```

A key can be:

- **a symbol**, held or not. A symbol you don't hold yet gets a buy line.
- **a sector**, such as `Technology` or `"Financial Services"`. The sector's
  holdings share it in proportion to their value.
- **an asset class**: `equity`, `debt`, `gold`, `international` or `cash`. The
  class of each holding comes from the same look-through as `portfolio xray`
  (gold ETFs and SGBs are gold, liquid funds are cash, shares in another currency
  are international).

Narrower targets are carved out of wider ones, never added on top:
`Technology 40` with `TCS 15` gives the other technology holdings 25%. If the
targets add up to less than 100%, the rest is shared equally among holdings no
target covers; if they add up to more, they are scaled down. A key that matches
nothing (a misspelt sector, say) is reported, not silently dropped. Remove one
with `stockwise portfolio target remove TCS`.

With no targets saved, StockWise rebalances to equal weights. The profile's
`max_stock_weight` is a hard limit for single stocks (not ETFs, funds or gold
bonds): a target above it is capped and the excess goes to the other holdings.

## 2. See where you stand

```bash
stockwise portfolio target drift
```

```text
  Target              Now%  Target%     Drift     Band   Status
  ──────────────────────────────────────────────────────────────
  INFY.NS            21.2%    30.0%     -8.8p     5.0p    UNDER
  SBIN.NS            60.9%    40.0%    +20.9p     5.0p     OVER
  TCS.NS             17.9%    30.0%    -12.1p     5.0p    UNDER
```

The band follows the 5/25 rule: 5 percentage points, or 25% of the target,
whichever is smaller (a 10% target has a ±2.5 point band). `stockwise portfolio
target drift 2.0` saves your own absolute band instead of 5. `checkup` reports
drift from the same numbers.

## 3. Add new money without selling

```bash
stockwise rebalance --inflow 50000
```

```text
  Symbol          Current   Now% Target% After%     Action        Value
  TCS.NS        ₹14,991.00  17.9%  30.0%  29.9%  BUY 10 shares  ₹24,985.00
  INFY.NS       ₹17,721.00  21.2%  30.0%  30.0%  BUY 18 shares  ₹22,370.40
  SBIN.NS       ₹50,925.00  60.9%  40.0%  38.1%  BUY 2 shares    ₹2,529.80

  Net cash      needs ₹49,885.20
  Cash left     ₹114.80
```

- **Now%** is the current weight, **Target%** the weight you set, **After%**
  the weight after the orders, from the rounded share counts.
- The new money is split to minimize the squared distance from target: the
  holdings furthest below fill first. Whole shares are rounded down, then the
  cash left buys more of the holding furthest below target while a share
  brings it closer. What is left is less than one share.
- Nothing is sold, so there is no capital gain to report.

`--cash-buffer 5000` keeps ₹5,000 of the new cash aside.

## 4. Rebalance by selling too

Without `--inflow`, holdings above target are sold and the proceeds pay for the
buys. `--inflow 50000 --with-sells` does the same with new cash added.

```bash
stockwise rebalance
stockwise rebalance --drift 2.0      # ignore holdings within 2 percentage points of target
```

Each sell shows the lots it takes, its short- and long-term gain, and the tax it
adds to this year's estimate (after this year's gains, the ₹1.25 lakh exemption
and 4% cess). Indian shares and funds sell oldest first, as a demat account
does. For US shares a sale takes losing lots first, then the lots with the
lowest tax, and leaves lots that turn long-term within 45 days for last; the
plan prints the `portfolio sell … --lots` command that records it and what the
lot choice saves against first-in, first-out.

Each trade also shows its costs: STT, stamp duty, exchange and SEBI fees with
GST and the DP charge for NSE/BSE trades, stamp duty on fund purchases, an FX
spread (`--fx-spread`, 0.5% by default) on trades in another currency, and
`--brokerage` per order if your broker charges one. Orders under `--min-trade`
(₹1,000 by default) are left out. A holding with no price is left out with a
warning; the weights are then of the other holdings.

## 5. Raise cash at the lowest tax

```bash
stockwise rebalance --raise 50000
```

Sells only, until ₹50,000 is raised after costs: losses first, then long-term
gains the exemption still covers, then long-term, then short-term gains, the
most overweight holding first among equals. Nothing is bought.

Output from a sample portfolio of nine large caps on 21 Nov 2026 (trimmed):

```text
  Raise ₹50,000.00 (sell only, lowest tax first)
  Total Value                  ₹5,37,572.50

  Symbol              Current    Now% Target%  After%             Action        Value       Tax    Costs
  TCS.NS           ₹59,405.00   11.1%   11.1%   12.2%                 OK
  INFY.NS          ₹60,390.00   11.2%   11.1%    2.0%     SELL 46 shares   ₹50,508.00     ₹0.00   ₹68.33
                   lot 1 (2025-01-15): 46 × ₹1,500.00 cost, -₹18,492.00 long-term
  ...

  Sells                        ₹50,508.00
  Buys                         ₹0.00
  Costs                        ₹68.33 (STT, stamp duty, exchange fees, DP, brokerage, FX spread as they apply)
  Net cash                     frees ₹50,439.67
  Cash left                    ₹439.67
  Tax on sells                 ₹0.00 (STCG +₹0.00, LTCG -₹18,492.00; incl. cess, after this year's gains and exemption)
  Raised                       ₹50,439.67 after costs; ₹50,439.67 after the tax on it
```

Read the bottom lines:

- **Costs**: the trading charges of every sell (STT, stamp duty, exchange and
  SEBI fees with GST, the DP charge, and `--brokerage` if you set it).
- **Tax on sells**: the tax these sells add to this year's estimate, after the
  gains you already booked this year, the ₹1.25 lakh LTCG exemption and the
  4% cess. A loss shows as a negative gain and lowers the tax.
- **Raised**: the cash after costs, and after the tax on it.

Here a holding at a loss covers the whole amount, so the tax is nil. Each
sell row names the lots it takes and their gain. A holding left above the
profile's single-stock limit gets a note above the table. Preview a sell with
`stockwise portfolio sell INFY 46 --preview`; after you place it at your
broker, record it with the same command without `--preview`.

Combine it with a cash buffer, or ask for JSON:

```bash
stockwise rebalance --raise 50000 --cash-buffer 5000
stockwise rebalance --raise 50000 --json
```

## 6. Try a plan without saving it

Pass targets on the command line to compare options. Nothing is saved.

```bash
stockwise rebalance TCS:30 INFY:30 SBIN:40 --inflow 50000
stockwise rebalance equity:70 gold:10 international:20
```

## 7. Keep the plan current

After you place the orders at your broker, record them so the next run starts
from the right place:

```bash
stockwise portfolio add TCS 10 2498.50          # a purchase dated today
stockwise portfolio sell AAPL 8 --lots 2        # a US sale from lot 2
stockwise import kite tradebook-latest.xlsx     # or import the new tradebook
```

## Next steps

- [Import your trades](guide-portfolio.md) if holdings are missing or
  out of date.
- [File your Indian capital-gains tax](guide-tax.md) before a rebalance that
  sells; `stockwise harvest` lists losses you could book first.
- Use the numbers elsewhere: `stockwise rebalance --inflow 50000 --json` prints
  the same plan as JSON. See [Script it with JSON, CSV and
  jq](guide-scripting.md).
- Every option is in the commands reference under
  [Rebalance](commands.md#rebalance). A review prompt for assistants is in
  [Use StockWise from Claude, Gemini, Codex or Cursor](guide-mcp.md).
