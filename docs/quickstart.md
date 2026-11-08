# Quickstart: five minutes

This page takes you from nothing to a quote, a portfolio with XIRR and a tax
estimate. It assumes you have not installed StockWise yet. Each step says how
long it takes.

## 1. Install and check (1 minute)

```bash
curl -fsSL https://raw.githubusercontent.com/iamsaquib8/stockwise/main/install.sh | sh
stockwise init      # pick a market, import holdings, optional AI assistant
stockwise doctor
```

The installer works on macOS, Linux and Termux. For Windows, Docker, Nix,
`.deb`, `.rpm` or `cargo`, see [Install](install.md).

`stockwise doctor` checks your setup and prints the folder where StockWise
keeps your data. If it reports a problem, see [Troubleshooting](troubleshooting.md).

To try the commands below without touching your real data, point StockWise at a
temporary folder first:

```bash
export STOCKWISE_HOME=$(mktemp -d)
```

Close the terminal, or run `unset STOCKWISE_HOME`, to go back to your real data.

## 2. Look up a stock (30 seconds)

```bash
stockwise quote RELIANCE TCS     # Indian stocks are the default
stockwise quote AAPL NVDA -m us  # add -m us for US stocks
stockwise TCS                    # no command needed: a one-screen overview
```

Each stock prints a short block: price, change, day range, 52-week range,
volume, market cap and P/E. Data comes from Yahoo Finance, so quotes can be
delayed. A bare ticker such as `TCS` means `TCS.NS` on the NSE.

## 3. Build a portfolio (2 minutes)

You can type a purchase, or import a file from your broker.

```bash
stockwise portfolio add TCS 10 3500 --date 2023-04-12   # 10 shares at ₹3,500
stockwise portfolio add INFY 20 1300 --date 2023-06-05
stockwise portfolio                                      # holdings, P&L and XIRR
```

Every purchase is a lot with its own date, so the holding period and the XIRR
are exact. To load real trades, use the guide
[Import your trades and see true returns](guide-portfolio.md). It covers
Zerodha, Groww, Upstox, INDmoney and mutual-fund CAS files.

## 4. Get a tax estimate (30 seconds)

```bash
stockwise portfolio sell TCS 4 3900   # record a sale; the oldest lots go first
stockwise tax in                      # LTCG and STCG for this financial year
```

`tax in` shows realised gains from your sales, and what you would owe if you
sold everything today. It is an estimate. The guide
[File your Indian capital-gains tax](guide-tax.md) explains how to read it and
how to export the rows for your ITR.

## 5. Connect an AI assistant (1 minute, optional)

```bash
claude mcp add stockwise -- stockwise mcp
```

This adds StockWise to Claude Code. Claude Desktop and Cursor need a small JSON
entry instead; see [Use StockWise from Claude or Cursor](guide-mcp.md). The
server only reads. It cannot place orders.

## Next steps

- Pick a goal on [What do you want to do?](guides.md).
- Keep the [cheatsheet](cheatsheet.md) open for the commands people use most.
- Look up an option in the [commands reference](commands.md).
