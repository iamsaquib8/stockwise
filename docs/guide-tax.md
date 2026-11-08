# File your Indian capital-gains tax

This guide takes the lots and sales in your portfolio and turns them into a
capital-gains estimate, a list of what you could book before 31 March, an
advance-tax schedule, and a CSV laid out for ITR Schedule CG and Schedule 112A.

You need: a portfolio with dated purchases and sales. If you have not built one
yet, start with [Import your trades](guide-portfolio.md).

StockWise gives an **estimate**. It knows only the sales it recorded, at the
prices it recorded, without brokerage or STT. Compare every figure with your
broker's tax P&L before you file, and ask a chartered accountant if you are
unsure.

## When to use which command

| Time of year | Question | Command |
|---|---|---|
| Any time | What is my tax so far this year? | `tax in` |
| Before 31 March | Which sales would book a loss, or use my unused exemption? | `harvest` |
| 15 Jun, 15 Sep, 15 Dec, 15 Mar | How much advance tax is due, and did I pay enough? | `tax advance` |
| When you file | What do I enter in Schedule CG and Schedule 112A? | `tax export in` |

## 1. Estimate the tax so far

```bash
stockwise tax in
```

```text
  Realized — sales 01 Apr 2026 to 31 Mar 2027
  Symbol               Qty Bought       Sold                 Gain  Term
  INFY.NS             5.00 2023-06-05   2026-07-10         +₹1000    LT

  Unrealized — if you sold everything today
  Symbol                 Cost        Value   Short-term    Long-term
  TCS.NS               ₹19500       ₹14991          +₹0       -₹4509
  ...

  Estimated tax on gains realized in FY 2026-27
  Long-term gains / losses     ₹1000.00 / ₹0.00
  LTCG exemption (₹125000 a year) -₹1000.00
  Taxable LTCG                 ₹0.00
  TOTAL TAX                    ₹0.00
```

The output has three blocks:

1. **Realized.** Every lot you sold this financial year, with its own holding
   period. Long-term means more than 12 months.
2. **Unrealized.** What each holding would book if you sold everything today.
   Nothing is sold.
3. **Estimated tax.** The set-off rules, the ₹1.25 lakh LTCG exemption, the
   rates and the 4% cess applied to the realized gains.

The rates follow the date of each sale. Sales before 23 July 2024 use the old
rates (15% STCG, 10% LTCG). Later sales use 20% and 12.5%.

Some holdings are left out of the estimate and listed with the rule that
applies: debt funds, Sovereign Gold Bonds, gold and silver ETFs, liquid ETFs and
crypto. Equity funds and equity ETFs count like listed shares. For the full
list, see [Lots, sales and tax](commands.md#lots-sales-and-tax).

## 2. Look at harvest candidates before 31 March

```bash
stockwise harvest
```

```text
  Loss Harvesting Candidates (STCG / LTCG Offset)
  Symbol             Sell   Avg Cost      Price       ST P&L       LT P&L    Total P&L  Est. Saved
  TCS.NS                6   ₹3250.00   ₹2498.50          +₹0       -₹4509       -₹4509          ₹0

  Tax-Gain Harvesting (Section 112A ₹1.25L Annual LTCG Exemption)
  Unused LTCG Exemption        ₹124000 of ₹125000 remaining this FY
  • SBIN.NS      LTCG:    +₹19925 (saves up to ₹2590 at 12.5%)
```

`harvest` lists two things:

- **Loss candidates.** Sales that would book a loss to set off against gains.
  Indian demat accounts sell the oldest lots first, so a row shows the whole
  sale needed to reach the losing lot. `Est. Saved` is how much the tax
  estimate falls. It is `0` when you have no gains to set the loss against.
- **Exemption candidates.** Long-term holdings with a gain, and how much of
  this year's ₹1.25 lakh LTCG exemption is still unused.

This is a list of what the arithmetic allows. It is not advice to trade.
Brokerage, STT and your own view of each stock are not in the numbers.

## Preview a sale before you record it

`portfolio sell` records the sale. Add `--preview` (alias `--dry-run`) to see
what it would do and change nothing: `portfolio.json` is not touched.

```
stockwise portfolio sell TCS 12 4000 --preview
```

The preview prints:

- **The departing lots**, oldest first (FIFO): lot number, purchase date,
  days held, quantity, cost, gain or loss, and ST or LT. Lot numbers match
  `portfolio lots`.
- **The tax the sale adds** on top of gains already recorded this tax year,
  from the same rules as `tax in`: short-term gain at 20%, long-term gain at
  12.5% after the ₹1.25 lakh Sec. 112A exemption still unused this year, the
  31 Jan 2018 grandfathered cost for old lots, and 4% cess. Other countries
  use their own rates. Funds and ETFs taxed under other rules show lots and
  gains only, with the reason.
- **A long-term countdown.** A lot within 45 days of turning long-term is
  listed with the day it turns and how much lower the tax would be if the
  same sale happened then, at the same price. A holding of exactly 12 months
  is still short-term; the first long-term day is the next one.
- **A mutual-fund exit-load check.** Units held under 365 days are flagged
  with a typical 1% exit load. Schemes differ, so check the factsheet.

The price defaults to the live quote (a fund's latest AMFI NAV). A past
`--date` needs a price. The same preview is the MCP tool
`portfolio_sell_preview`. These are arithmetic on your records, not advice.

## Regular vs Direct fund cost

```
stockwise mf bleed [--return 10] [--json]
```

For each held Regular-plan growth fund, `mf bleed` finds the Direct growth
plan of the same scheme and reads both expense ratios from AMFI's TER
disclosure. It shows the gap, the yearly rupee cost of the gap on today's
value, and the 10-year compounded difference at a gross return you choose
(`--return`, default 10% a year, no new money). It then splits the units by
age:

- Units held more than 12 months, and how many of them redeem with ₹0 tax
  inside this year's unused Sec. 112A exemption (oldest units first).
- Younger units, with the day each lot turns long-term and, for a switch
  today, the tax, the exit load and the months of TER saved needed to cover
  them.

A switch is a redemption plus a purchase, so it can create tax. Funds that
are not equity-oriented are shown without the tax split. `portfolio` prints
a one-line pointer when you hold a Regular plan. MCP tool: `mf_bleed`.

## 3. Check advance tax

Advance tax is due when the estimated tax for the year is ₹10,000 or more.
Pass each payment with its date so StockWise can work out Section 234C
interest exactly.

```bash
stockwise tax advance                                # the current financial year
stockwise tax advance --fy 2025-26 --paid 2025-09-10:500
```

The second command gives output like this:

```text
  Quarter Due Date       Cum. STCG    Cum. LTCG     Cum. Tax  Min %  Min Cum. Due   Quarter Due  Status
  Q1      15 Jun 2025           ₹0           ₹0           ₹0    15%            ₹0            ₹0  Nothing due
  Q2      15 Sep 2025           ₹0         ₹200           ₹0    45%            ₹0            ₹0  Nothing due
  ...
  • Section 208: Exempt (estimated tax < ₹10,000 threshold — advance tax not mandatory)
  • Advance Tax Paid Recorded: ₹500
  • Estimated Section 234C Interest: ₹0 (no shortfall subject to interest)
```

Things to know:

- Add `--fy 2025-26` for a past year.
- `--paid 25000` with no date is assumed to be on time. That can understate
  the interest, so use dates when you have them.
- Capital gains realised after an instalment date do not attract 234C interest
  if you pay the tax in the later instalments. StockWise applies this proviso.
- Surcharge is not included.

## 4. Export the schedule for your ITR

```bash
stockwise tax export in --fy 2025-26 --out cg-2025-26.csv
```

```text
  Capital-gains schedule — FY 2025-26 (AY 2026-27)
  Sales                        1
  STCG 111A gains / losses     ₹0 / ₹0
  LTCG 112A gains / losses     ₹200 / ₹0
  Taxable LTCG (112A)          ₹0
  Estimated tax                ₹0

  Wrote 1 sales to cg-2025-26.csv
```

The CSV has one row per lot sold: section (111A or 112A), ISIN, scrip,
quantity, sale date and value, purchase date and cost, days held and the gain.
A summary block follows with the totals, the exemption and the loss to carry
forward. `--fy` accepts `2025-26`, `2025-2026` and `AY2026-27`.

Open the file in a spreadsheet and compare it with your broker's tax P&L. Check
these before you file:

- **Shares bought on or before 31 January 2018.** StockWise flags them.
  Schedule 112A lets you use that day's fair market value as the cost. StockWise
  does not apply it.
- **ISINs.** StockWise looks them up from NSE and AMFI. A blank ISIN is named
  in the summary.
- **Same-day trades.** A buy and sell of the same share on one day is shown
  separately as speculative business income, not as capital gains.
- **Left out.** Debt funds, SGBs, gold ETFs and crypto are listed but not in the
  schedule. Crypto is taxed at a flat 30% under Section 115BBH.

## Next steps

- [Rebalance to target weights](guide-rebalance.md). A buy-only rebalance adds
  money without selling, so it books no gain.
- [Use StockWise from Claude or Cursor](guide-mcp.md). The `tax_harvest_check`
  prompt asks an assistant for the same review.
- [Script it with JSON, CSV and jq](guide-scripting.md), for example
  `stockwise tax export in --fy 2025-26 --json | jq '.summary.taxable_ltcg'`.
- Every rule and field is in the commands reference:
  [lots, sales and tax](commands.md#lots-sales-and-tax),
  [harvest](commands.md#harvest), [tax export](commands.md#tax-export) and
  [advance tax](commands.md#advance-tax-india).
- Numbers look wrong? See [An import skips a
  symbol](troubleshooting.md#an-import-skips-a-symbol).
- StockWise is not financial or tax advice. See the [disclaimer](disclaimer.md).
