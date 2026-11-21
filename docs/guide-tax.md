# File your Indian capital-gains tax

This guide takes the lots and sales in your portfolio and turns them into a
capital-gains estimate, a list of what you could book before 31 March, an
advance-tax schedule, a CSV laid out for ITR Schedule CG and Schedule 112A,
a check against your broker's tax P&L, your dividend and interest income for
Schedule OS, Schedule FA for foreign shares, and an audit pack with one
working paper per sale.

You need: a portfolio with dated purchases and sales. If you have not built one
yet, start with [Import your trades](guide-portfolio.md).

StockWise gives an **estimate**. It knows only the sales it recorded, at the
prices it recorded, without brokerage or STT. Compare every figure with your
broker's tax P&L before you file (`tax reconcile` does it sale by sale), and
ask a chartered accountant if you are unsure.

## When to use which command

| Time of year | Question | Command |
|---|---|---|
| Any time | What is my tax so far this year? | `tax in` |
| Before 31 March | Which sales would book a loss, or use my unused exemption? | `harvest` |
| 15 Jun, 15 Sep, 15 Dec, 15 Mar | How much advance tax is due, and did I pay enough? | `tax advance` |
| When you file | What do I enter in Schedule CG and Schedule 112A? | `tax export in` |
| When you file | Does my broker's tax P&L agree with StockWise? | `tax reconcile FILE` |
| When you file | What dividend and interest income, and how much TDS? | `tax income` |
| When you file | I hold US shares: Schedule FA and Form 67? | `tax foreign` |
| Any time | Show my CA how every gain was worked out | `tax export in --audit-pack DIR` |

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

The **next instalment** is shown under the table: the cumulative amount due by
that date, what you have paid, what is left and the Section 234C interest if
you miss it. On 21 November 2026, with ₹4 lakh of short-term gains booked in
May, it looks like this:

```text
  ── Next instalment — Q3 due 15 Dec 2026 ──
  Due                          in 24 days
  Cumulative by 15 Dec         ₹62,400
  Paid so far                  ₹0
  To pay                       ₹62,400
  Sec. 234C interest if missed ₹1,872
```

Two flags help:

- `--paid 2026-12-10:62400 --record` saves the payment, so later runs (and the
  reminder) count it without `--paid`.
- `--slab 30` adds your dividends and SGB interest (see step 6) at your slab
  rate, each in the quarter it was received. Dividends share the capital-gains
  proviso, so a dividend received in January raises only the March instalment.

StockWise can remind you 7 days before a due date when something is still to
pay, through the same notifiers as alerts.

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

- **Shares bought on or before 31 January 2018.** StockWise uses the
  grandfathered cost: the higher of the actual cost and the lower of the 31
  January 2018 price and the sale price (Section 112A). Check the price your
  broker used: `tax reconcile` flags a difference.
- **ISINs.** StockWise looks them up from NSE and AMFI. A blank ISIN is named
  in the summary.
- **Same-day trades.** A buy and sell of the same share on one day is shown
  separately as speculative business income, not as capital gains.
- **Left out.** Debt funds, SGBs, gold ETFs and crypto are listed but not in the
  schedule. `tax in` shows the rule for each (below).

### Debt funds, gold ETFs and crypto

These are not taxed like listed shares, and `tax in` gives them a section of
their own:

- **Section 50AA.** Debt funds and liquid, gilt or debt ETFs bought on or after
  1 April 2023, and market-linked debentures, are short-term whatever the
  holding period: the gain is added to your income at your slab rate.
- **Section 112.** Gold, silver and international funds and ETFs, older debt
  fund units and SGBs sold on the exchange are long-term after 12 months
  (listed) or 24 months (unlisted), at 12.5% with no ₹1.25 lakh exemption.
- **Section 115BBH.** Crypto is taxed at 30% on each gain. No loss is set off,
  not even against another crypto gain, and none is carried forward. The 1% TDS
  deducted on each sale is shown so you can match it with Form 26AS.

StockWise tells a gold, silver, international or debt ETF from an equity ETF by
its category in NSE's ETF list, so a new fund is recognised the day it lists.

## 5. Reconcile with your broker's tax P&L

Download the tax P&L or capital-gains report from your broker (Zerodha
Console → Reports → Tax P&L, Groww → Reports → Capital gains, Upstox →
Reports → Capital gain) as `.xlsx` or CSV, and compare:

```bash
stockwise tax reconcile ~/Downloads/taxpnl-FY2026-27.xlsx
```

```text
  Tax Reconcile — StockWise vs Zerodha — FY 2026-27
  3 matched (0 agree, 3 differ) · 3 only in StockWise · 1 only in Zerodha

  Differences
  Symbol       Sold              Qty     StockWise       Zerodha        Diff  Likely reason
  INFY         2026-05-12         10        ₹1,500        ₹1,469         ₹31  charges counted on one side only
  TCS          2026-06-02          5        ₹2,000        ₹2,000          ₹0  buy date differs
               bought 2025-01-15 (StockWise) vs 2025-01-10 (Zerodha)
  HDFCBANK     2026-07-20         10       ₹11,000        ₹8,520      ₹2,480  grandfathering (31 Jan 2018 FMV) applied differently

  Totals
                  StockWise        Zerodha     Difference
  STCG              ₹47,300         ₹1,869        ₹45,431
  LTCG              ₹13,000        ₹10,520         ₹2,480
  Intraday             ₹500             ₹0           ₹500
```

Each difference gets a likely reason: a quantity gap (a missing buy, or a split
or bonus not applied — run `stockwise portfolio actions`), a different buy date
or cost, grandfathering on pre-2018 shares, or the broker's charges. Sales on
one side only are listed, and the totals compare STCG, LTCG and intraday. Only
StockWise sales from that broker's account are compared unless you add
`--all-accounts`. `--json` gives the same for scripts. StockWise never changes
the portfolio here.

## 6. Dividend and interest income

```bash
stockwise tax income --fy 2026-27
stockwise tax income --fy 2026-27 --out schedule-os-2026-27.csv
```

Dividends come from your holdings' corporate actions (run `stockwise portfolio
actions --apply` first), SGB interest from the 2.5% coupon on the units you
held on each payment date. Each line shows gross, TDS and net, and the ITR
period — the same periods as advance tax. TDS is 10% once one company's
dividends to you pass ₹10,000 in the year; SGB interest has none. Match the TDS
with Form 26AS or AIS: a Form 15G/H or a missing PAN changes it.

## 7. Foreign shares: Schedule FA and Form 67

If you hold US shares or RSUs, Schedule FA asks for each holding over the
**calendar year** (January to December): the initial value, the peak value,
the closing value on 31 December, dividends and sale proceeds, in rupees.

```bash
stockwise tax foreign --year 2025
stockwise tax export in --schedule fa --fy 2025-26 --out schedule-fa-2025.csv
stockwise tax export in --schedule form67 --fy 2025-26 --out form67.csv
```

The peak value uses each trading day's close and that day's rupee rate. Form 67
lists the US tax withheld on each dividend (25%) and the credit you can claim:
the lower of that tax and the Indian tax on the same income at `--slab`. File
Form 67 before the ITR. StockWise uses the ECB reference rate; the rules ask
for the SBI TT buying rate, so expect small differences.

## 8. Keep an audit pack

```bash
stockwise tax export in --fy 2026-27 --audit-pack ~/tax/2026-27-audit
```

The folder gets one CSV per sale — lot, dates, cost, the 31 January 2018 fair
market value where it applies, the gain and the rule applied — a
`summary.csv` with totals by rule, and the Schedule CG CSV. Give it to your CA
with your broker's statements.

## Next steps

- [Rebalance to target weights](guide-rebalance.md). A buy-only rebalance adds
  money without selling, so it books no gain.
- [Use StockWise from Claude, Gemini, Codex or Cursor](guide-mcp.md). The `tax_harvest_check`
  prompt asks an assistant for the same review.
- [Script it with JSON, CSV and jq](guide-scripting.md), for example
  `stockwise tax export in --fy 2025-26 --json | jq '.summary.taxable_ltcg'`.
- Every rule and field is in the commands reference:
  [lots, sales and tax](commands.md#lots-sales-and-tax),
  [harvest](commands.md#harvest), [tax export](commands.md#tax-export),
  [audit pack](commands.md#audit-pack), [tax reconcile](commands.md#tax-reconcile),
  [income](commands.md#dividend-and-interest-income-schedule-os),
  [Schedule FA](commands.md#foreign-assets-schedule-fa-and-form-67) and
  [advance tax](commands.md#advance-tax-india).
- Numbers look wrong? See [An import skips a
  symbol](troubleshooting.md#an-import-skips-a-symbol).
- StockWise is not financial or tax advice. See the [disclaimer](disclaimer.md).
