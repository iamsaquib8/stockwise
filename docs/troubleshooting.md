# Troubleshooting

Start with `stockwise doctor`. It checks the config file, the data directory,
whether Yahoo Finance answers, the optional Redis cache, the AI model and your
broker config, and exits with status 1 if market data is unreachable or the
config file is invalid.

Walkthroughs for the common jobs are in [What do you want to do?](guides.md).

The messages below are quoted from StockWise. `...` stands for text that
changes from run to run.

## Where is my data?

Your portfolio, watchlist, alerts and caches are plain files in one folder.
`stockwise doctor` prints it (the line "Data directory").

| System | Default folder |
|--------|----------------|
| macOS | `~/Library/Application Support/stockwise/` |
| Linux | `~/.local/share/stockwise/` |
| Windows | `%LOCALAPPDATA%\stockwise\` |

`STOCKWISE_HOME` (or `home` in the [config file](commands.md#configuration-file))
chooses another folder. Use a scratch folder to try things without touching your
portfolio: `export STOCKWISE_HOME=$(mktemp -d)`. The files are listed in
[Data storage](architecture.md#data-storage). `cache/` is safe to delete.
A named portfolio (`--portfolio NAME`) lives in `portfolios/NAME.json`.

## Yahoo Finance is unreachable or throttled

Most prices and history come from Yahoo Finance. Three messages come from it:

- `Cannot reach Yahoo Finance (...). Check your internet connection.` The
  request did not get an answer. Check the network, a VPN or a proxy, then run
  `stockwise doctor`.
- `Yahoo Finance is rate limiting requests. Wait a minute and try again.` Yahoo
  answered HTTP 429. StockWise already retries three times and honours
  `Retry-After`, so run fewer commands at once (a loop over many symbols is the
  usual cause) and try again in a minute. Redis, if you run it, caches answers
  across commands.
- `Yahoo Finance refused to start a session (HTTP ...)` or `Yahoo Finance
  returned HTTP ... after 3 attempts`. Yahoo is having a problem. Wait and retry,
  or see [data providers](data-providers.md).

## NSE refuses automated access (HTTP 403)

NSE's website serves browsers and often blocks scripts. StockWise opens one polite
session, spaces its requests, never retries a refusal and does not try to get
around it. You see one of:

- `NSE refused the option-chain request (HTTP 403). NSE's API serves browser
  sessions and sometimes blocks scripts — wait a minute and retry, or open
  nseindia.com/option-chain. ...` Affects `options chain`, `options pcr`,
  `options oi` and `options maxpain` for Indian symbols. US options (`-m us`) use
  Yahoo instead. BSE has the same kind of message (`BSE refused the
  option-chain request ...`).
- `NSE refused the session (HTTP 403)`, `NSE refused the request (...)` or `NSE
  refused an earlier request; not asking again for now`. After a refusal
  StockWise stays away from that site for a while (15 minutes for most
  features, an hour for the calendar), so retrying at once does not help.
  Long-running `serve`, `tui` and `daemon` processes try again after that.
- `NSE refused the request (HTTP 403); ... Showing Yahoo Finance dates only` in
  `calendar`: you still get Yahoo's dates, without NSE's board meetings and
  corporate actions.

Features that read NSE lose that data while it refuses: `ipo`, the filings in
`events`, `calendar`, `--universe` scans (a built-in list is used, with a note
on stderr), SGB prices (the last saved list is shown, or no price) and the NSE
data provider. Waiting a while, or a different network, usually fixes it.

## AMFI file unavailable

Mutual fund NAVs come from AMFI's `NAVAll.txt`, cached in `cache/` for 12
hours. If the download fails:

- With an older copy in `cache/`, the command still works and prints
  `Warning: ...; using the copy saved <date> from <path>` on stderr.
- Without one, you get `AMFI returned HTTP ...`, `Cannot reach AMFI (...)` or
  `AMFI's NAV file had no schemes; try again later`.

`mf search`, `mf nav`, and `portfolio`, `tax in` and `harvest` for funds you
hold all need that file (`mf history` and `mf sip` also use api.mfapi.in). A fund that AMFI does not list stays `N/A`
with a note, never 0. Expense ratios (TER) come from a second AMFI download;
when it fails, NAVs and returns still print with a note, but `mf screen -s ter`
and `--max-ter` stop with an error.

## `Not enough price data`

```
Error: Not enough price data for technical analysis — try a longer period, e.g. -p 6mo
```

An indicator needs enough daily bars: `technical` needs 30, so `-p 1mo`
(about 21 bars) is too short, and `risk` and `forecast` have similar limits.
Use a longer period (`-p 6mo`, `-p 1y`). A newly listed stock may not have
enough history at all.

## Unknown ticker

```
Error: No quote for RELIANC.NS (Indian market is the default — for US stocks add `-m us`; use `stockwise search <name>` to find a ticker)
  Did you mean: RELIANCE.NS (Reliance Industries)
```

- A bare ticker is Indian (`TCS` means `TCS.NS`). For a US stock add `-m us`
  (`stockwise quote AAPL -m us`), or set `STOCKWISE_MARKET=us`.
- `stockwise search "company name"` finds the ticker.
- `portfolio add`, `watch add` and `alert add` refuse a ticker Yahoo does not
  know and save nothing: `Nothing was saved. Add --force to keep it anyway.` Use
  `--force` only when you are sure, for example for a delisted stock.
- A mutual fund is `MF:<AMFI code>` or `MF:<ISIN>`, not a ticker (`stockwise mf search`
  finds the code).

## `--yes` needs `max_order_value`

```
--yes skips the typed confirmation, so it needs a cap on the order value. Add
"max_order_value": 50000 to <file> (your <broker> config), or leave out --yes and
confirm by typing yes. --dry-run always works.
```

`trade buy`, `trade sell` and the other order commands normally show a preview
and ask you to type `yes`. `--yes` skips that, and StockWise only allows it when
your broker config file has a `max_order_value`. Add the key (the message names
the file) or drop `--yes`. If the order's value is above the cap, or cannot be
estimated (a market order without a price), the order is refused: `Estimated
value ₹... exceeds max_order_value ₹... in your ... config`. Try the order with
`--dry-run` first. See [Brokers](brokers.md).

## An import skips a symbol

`stockwise import` prints one line per skipped symbol, and exits with status 1
(`Nothing imported from FILE — see the skipped holdings above.`) when it
updated nothing.

- `skipped: it has lots or sales from elsewhere (manual entries, another import,
  or a tradebook imported by an older version) that this file would replace.
  Re-run with --replace to rebuild it from this file.` A dated file (a tradebook
  or a list of lots) rebuilds a symbol's lots, so StockWise will not do it over
  lots you entered another way. Run the import again with `--replace` if the
  file should win. Check first with `stockwise portfolio lots SYMBOL`.
- `not applied yet: sells of N shares have no earlier buy in the trades
  imported so far.` A tradebook sold shares that an earlier file bought. Import
  the earlier tradebook too (the trades of this file are kept, in any order),
  or use `--replace` to rebuild from this file alone.
- A holdings file with no dates only changes quantities: more shares add a lot
  dated today, fewer take from the oldest lots. A split or bonus that is not applied
  yet blocks it; run `stockwise portfolio actions --apply` first.
- An unrecognised date is refused, and an `.xlsx` workbook with several sheets
  uses the first one whose header fits (`--sheet NAME` picks another).

More in [importing holdings](brokers.md#importing-holdings) and
[the portfolio commands](commands.md#lots-sales-and-tax).

## The config file is invalid

`Invalid config file <path>: ...` stops every command with exit code 1. `config`,
`doctor` and `completions` still run. Fix the key it names, or write a fresh
template with `stockwise config init --force`.

## Still stuck?

Run the command with `--json` (errors go to stderr, so stdout stays clean) or
check the [commands reference](commands.md). To report a bug, include the
command, its full output and `stockwise --version` in an
[issue](https://github.com/iamsaquib8/stockwise/issues/new/choose). Do not paste
files from your data folder, broker config or API keys.
