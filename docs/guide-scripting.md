# Script it with JSON, CSV and jq

This guide shows how to take StockWise output into a spreadsheet, a shell
script, a cron job or another program. It takes about five minutes.

You need: StockWise, and [jq](https://jqlang.github.io/jq/) for the JSON
examples. A spreadsheet handles the CSV.

## 1. Choose a format

Many commands take `--json`, and commands that print tables also take `--csv`.
`stockwise <command> --help` shows which flags a command has. You cannot
combine the two.

```bash
stockwise portfolio --json   # one JSON document
stockwise portfolio --csv    # a header row, then one row per holding
```

Both go to stdout and contain nothing else. Progress messages and errors go to
stderr, so redirecting stdout is safe.

## 2. Open it in a spreadsheet

```bash
stockwise portfolio --csv > portfolio.csv
stockwise history TCS -p 1y --csv > tcs.csv       # every daily bar
stockwise screen dividend --csv | column -s, -t   # a quick view in the terminal
```

The CSV columns have the same names as the JSON keys.

```text
symbol,shares,avg_cost,cost_basis,price,value,pnl,pnl_percent,weight_percent,xirr_percent,currency,name,price_source,nav_date
TCS.NS,6.0,3250.0,19500.0,2498.5,14991.0,-4509.0,-23.12,17.92,-4.61,INR,TATA CONSULTANCY SERV LT,quote,
```

## 3. Pull one number with jq

```bash
stockwise portfolio --json | jq '.totals'
```

```text
{"cost_basis":70000.0,"value":83637.0,"pnl":13637.0,"pnl_percent":19.48,"currency":"INR"}
```

More patterns:

```bash
# The three worst holdings by return
stockwise portfolio --json | jq -r '.holdings | sort_by(.pnl_percent) | .[:3][] | "\(.symbol) \(.pnl_percent)%"'

# Estimated tax so far this year
stockwise tax in --json | jq '.realized.total_tax'

# Taxable long-term gain in a past year
stockwise tax export in --fy 2025-26 --json | jq '.summary.taxable_ltcg'

# Symbols of triggered alerts
stockwise alert check --json | jq -r '.[] | select(.status == "triggered") | .symbol'

# Prices only
stockwise quote TCS INFY --json | jq '.[] | {symbol, regularMarketPrice}'

# Your return against the index
stockwise portfolio benchmark --json | jq '{portfolio_return, benchmark_return, beta: .metrics.beta}'
```

The shape of every command's JSON is in the
[Shapes table](commands.md#scripting-with---json-and---csv).

## 4. Loop over symbols

```bash
# RSI and bias for everything on your watchlist
for s in $(stockwise watch --json | jq -r '.[].symbol'); do
  stockwise technical "$s" --json | jq -r '"\(.symbol)\t\(.rsi_14)\t\(.bias.verdict)"'
done

# Compare the built-in strategies on one stock
for s in rsi macd sma bb vwap mr; do
  stockwise backtest INFY -s "$s" --json | jq -r '"\(.strategy)\t\(.total_return_percent)"'
done
```

## 5. Rules that make scripts safe

- **Exit code.** Errors go to stderr and the command exits with status 1, so
  `set -e` and `||` work.
- **Empty is not an error.** An empty portfolio prints `{"holdings": [], ...}`
  or a CSV header with no rows.
- **Missing is `null`.** A value that is not available is `null` in JSON and an
  empty cell in CSV. It is never `0`. Test for `null` before you do arithmetic.
- **Names are stable.** Keys are `snake_case`. The exceptions are `quote` and
  `search`, which pass Yahoo's own camelCase fields through.
- **Percent fields.** A field ending in `_percent` is a percentage: `12.5` means
  12.5%. A `dividend_yield` is a fraction: `0.012` means 1.2%.
- **Dates** are `YYYY-MM-DD`. `history` bar times are RFC 3339 in UTC.
- **No AI text.** The `--ai` section is never part of JSON or CSV output.

## 6. Run it on a schedule

Save the day's portfolio value with `wealth`, and mail yourself a daily line:

```bash
# crontab: weekdays at 16:00
0 16 * * 1-5  stockwise wealth > /dev/null
5 16 * * 1-5  stockwise portfolio --json | jq -r '"Portfolio \(.totals.value) (\(.totals.pnl_percent)%)"' | mail -s "StockWise" you@example.com
```

Check alerts from cron with `stockwise alert watch --once`; see
[Set up alerts and the daemon](guide-alerts.md).

Save your defaults in a config file so scripts do not repeat flags:

```bash
stockwise config init     # writes a starter config file
stockwise config show     # where each setting comes from
```

Shell completion for zsh:

```bash
stockwise completions zsh > ~/.zfunc/_stockwise
```

## Next steps

- [Use StockWise from Claude or Cursor](guide-mcp.md) for the same data through
  an assistant.
- [Rebalance to target weights](guide-rebalance.md) and
  [File your Indian capital-gains tax](guide-tax.md) both print JSON you can
  feed into a sheet.
- The full list of JSON shapes and the rules above are in the
  [commands reference](commands.md#scripting-with---json-and---csv). The
  [configuration file](commands.md#configuration-file) section lists every
  setting.
- A web version of the same data: `stockwise serve --open`. See
  [Web dashboard and API](server.md).
