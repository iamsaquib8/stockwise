# Import your trades and see true returns

This guide loads your broker trades and mutual-fund statement into StockWise
and shows your holdings with profit and XIRR. It takes about ten minutes, most
of it spent downloading files.

You need: a Zerodha, Groww, Upstox or INDmoney export, and optionally a CAS
for mutual funds. Nothing here changes your broker account. StockWise reads
files and stores the result in local files.

## 1. Pick the right file

XIRR and tax need the **date** of every purchase. The file you choose decides
whether StockWise has it.

| File | Has dates? | What StockWise does |
|---|---|---|
| Tradebook (every buy and sell, such as Zerodha Console's) | Yes | Builds exact lots. Sales come out oldest-first and are kept for tax. |
| Holdings export (Kite, Groww, Upstox, INDmoney) | No | Matches quantity only. New shares are dated today, so holding periods and XIRR are approximate. |
| CAS from CAMS or KFintech, read with `casparser` | Yes, for funds | Rebuilds each fund as dated lots when it covers the whole balance. |
| Any CSV with a date column | Yes | Read as a list of lots. |

Use a tradebook when you can. Use a holdings export only for stocks you cannot
get a tradebook for.

Zerodha Console limits a tradebook download to 365 days. Download one file per
year. StockWise merges them.

## 2. Import the tradebook

Import each file with the broker name. CSV and `.xlsx` both work.

```bash
stockwise import kite ~/Downloads/tradebook-2023.xlsx
stockwise import kite ~/Downloads/tradebook-2024.xlsx
stockwise import kite ~/Downloads/tradebook-2025.xlsx
```

You can import the files in any order. StockWise keeps the trades of each
import and merges them, so running the same file twice changes nothing.

Each import prints one line per symbol:

```text
  ✓ TCS.NS — 1 lot(s), 1 sale(s) recorded, now 6 shares @ ₹3250.00
  ✓ INFY.NS — 1 lot(s), 1 sale(s) recorded, now 15 shares @ ₹1300.00
  ✓ SBIN.NS — 1 lot(s), now 50 shares @ ₹620.00

  ✓ Imported 5 rows: 3 holdings updated.
```

Splits and bonuses are applied as they happened. This needs a network
connection.

If a symbol is skipped, the message says why. The usual cause is a sale with
no earlier purchase, because an earlier year's file is missing. Import that
file and run the import again. See [An import skips a
symbol](troubleshooting.md#an-import-skips-a-symbol).

## 3. Import mutual funds from a CAS

A CAS is a PDF, and its tables do not survive text extraction. Convert it with
the open-source [casparser](https://github.com/codereverser/casparser), then
import the result.

```bash
pip install casparser
casparser CAS.pdf -p <password> -o cas.json
stockwise import cas cas.json
# or let StockWise run casparser for you (casparser must be on PATH):
stockwise import cas CAS.pdf --password <password>   # or STOCKWISE_CAS_PASSWORD, or a hidden prompt
```

Each fund with units left becomes `MF:<AMFI code>`. StockWise values it at
AMFI's latest end-of-day NAV. A fund that AMFI does not list shows `N/A`, never
zero.

## 4. See your portfolio

```bash
stockwise portfolio
```

```text
  Symbol         Shares   Avg Cost      Price        Value         P&L    P&L %     XIRR
  ──────────────────────────────────────────────────────────────────────────────────────
  TCS.NS           6.00   ₹3250.00   ₹2498.50    ₹14991.00   -₹4509.00  -23.12%    -4.6%
  INFY.NS         15.00   ₹1300.00   ₹1181.40    ₹17721.00   -₹1779.00   -9.12%    -0.9%
  SBIN.NS         50.00    ₹620.00   ₹1018.50    ₹50925.00  +₹19925.00  +64.27%   +19.3%
  ──────────────────────────────────────────────────────────────────────────────────────
  TOTAL                  ₹70000.00               ₹83637.00  +₹13637.00  +19.48%    +5.1%
  Realized P&L (2 sale lots)   +₹1200.00
```

The numbers above come from a small sample portfolio. Prices are live, so
yours will differ.

How to read it:

- **P&L %** is the gain on cost, ignoring when you bought.
- **XIRR** is the yearly return that accounts for the date of every purchase.
  A holding that gained 64% over 3 years has a much lower XIRR than 64%.
- **Realized P&L** is the profit on shares you already sold.
- A holding with no price shows `N/A` and is left out of the totals.

Mixed INR and USD holdings are totalled in one currency. Pick it with
`--base inr` or `--base usd`.

## 5. Check one holding

```bash
stockwise portfolio lots TCS
```

```text
    # Bought             Qty      Price         Cost     Held  Term  Source
    1 2023-04-12        6.00   ₹3250.00    ₹19500.00    3y 6m    LT  import:kite [zerodha]

  Sales (FIFO)
  Sold               Qty Bought              Buy       Sell         Gain  Term
  2025-08-20        4.00 2023-04-12     ₹3250.00   ₹3300.00     +₹200.00    LT
```

Each row is one lot with its date, its holding period (`LT` is long-term, `ST`
is short-term) and where it came from. Check these against your broker's
statement. If a purchase date is wrong, correct it with
`stockwise portfolio lots TCS --lot 1 --date 2023-04-10`.

## 6. Compare with the index and look through your funds

```bash
stockwise portfolio benchmark NIFTY -p 1y   # your return against NIFTY 50
stockwise portfolio xray                    # overlap across stocks, funds and ETFs
```

`benchmark` rebuilds what you held on each day and compares the
time-weighted return with the index. It also prints alpha, beta and tracking
error. `xray` adds up your real exposure to each company across direct shares,
ETFs and mutual funds, and flags any company above 10% of the portfolio.

## 7. Keep it up to date

- Import the newest tradebook after each year, or whenever you want. Old
  trades are not counted twice.
- Add a purchase by hand: `stockwise portfolio add TCS 10 3500 --date 2023-04-12`.
- Record a sale by hand: `stockwise portfolio sell TCS 5 3900`.
- Apply splits, bonuses and dividends to lots you added by hand:
  `stockwise portfolio actions --apply`. It is safe to repeat.
- Rebuild one symbol from a dated file, replacing lots recorded another way:
  `stockwise import kite tradebook.csv --replace`.

## Next steps

- [File your Indian capital-gains tax](guide-tax.md). The sales you imported
  feed straight into it.
- [Rebalance to target weights](guide-rebalance.md).
- Every portfolio option is in the [commands reference](commands.md#portfolio).
  The file layouts and rules for imports are in
  [Brokers](brokers.md#importing-holdings), and the
  [lots, sales and tax](commands.md#lots-sales-and-tax) section explains FIFO.
- Something looks wrong? See [Troubleshooting](troubleshooting.md).
