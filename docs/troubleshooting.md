# Troubleshooting

Start with `stockwise doctor`. It checks the config file, the data directory,
whether Yahoo Finance answers, the data providers, the cache, the AI model, the
keyring, your broker config, the notification channels, the worker login
service, the MCP config of your AI apps, and your tax residence and investor
profile. Each failed check prints a fix hint. It exits with status 1 if market
data is unreachable or the config file is invalid.

## See what StockWise is doing

| Tool | What it shows |
|------|---------------|
| `stockwise doctor` | One line per check: `✓` passed, `!` works with a limit, `✗` failed, each with its fix |
| `stockwise doctor --json` | The same checks as JSON, to paste into an issue or check in a script |
| `stockwise --verbose quote TCS` | Each network request on stderr: host, HTTP status, time, and whether the cache answered |
| `STOCKWISE_LOG=debug stockwise quote TCS` | The same diagnostics, with finer control: `debug` (everything), `info`, `warn` (problems only) |

The diagnostics go to stderr, so `--json` output on stdout stays clean. With
`--json`, an error is one JSON object on stderr,
`{"error":{"message":"...","exit_code":N}}`.

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

Most prices and history come from Yahoo Finance. `doctor` shows:

```
  Yahoo Finance                ✗ unreachable — check your internet connection or proxy
```

Three messages come from it in other commands:

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

Run the command with `--verbose` to see which host failed and with what status.
A keyed provider (`STOCKWISE_DATA_PROVIDER`) that fails shows in `doctor` as
`! <provider> did not answer (...); using Yahoo instead`; one with no key as
`! <provider> selected but <KEY> is not set; using Yahoo`.

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
`--verbose` shows the 403 answers with the NSE or BSE host. `hot` prints each
source that failed as a warning and shows the rest.

## Old or wrong data (stale cache)

StockWise caches answers in memory and in `cache.db` in the data directory (or
in Redis when `STOCKWISE_REDIS_URL` is set). Quotes live 30 seconds, charts up
to 10 minutes, results six hours, index members until cleared. If a number
looks stuck or wrong after a fix upstream:

```bash
stockwise cache status          # entries, size and location of cache.db
stockwise cache clear           # HTTP responses (the default)
stockwise cache clear quotes    # or snapshots, bars, results, members, bhavcopy
stockwise cache clear all       # everything in cache.db
```

`doctor` shows the cache as `Cache database ✓ cache.db (N entries, ...)`.
Clearing never touches your portfolio, watchlist, alerts or settings.
`--verbose` marks each request that the cache answered, so you can see whether
a figure came from the network.

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
`doctor` and `completions` still run. `doctor` shows:

```
  Config file                  ✗ <path> (invalid)
```

and the parse error under it. Fix the key it names, or write a fresh template
with `stockwise config init --force`. `STOCKWISE_CONFIG` pointing to a file
that does not exist is also an error (`create it with stockwise config init`).

## Tax residence or investor profile looks wrong

Tax rules, rebalance limits and the size hints on actions read your investor
profile and tax residence. `stockwise profile show` prints each setting and
where it came from (`default`, the config file or the environment). `doctor`
checks the profile and the residence and names a value it cannot use.

- Set the residence in the config file: `[profile] residence = "in"` (or
  `"us"`, `"uk"`). Without it, StockWise assumes the residence from the market
  and says so. Any other value is an error:
  `` `profile.residence`: expected `in`, `us` or `uk`, got `...` ``.
- Set the mode and guardrails with `stockwise profile set active:med` (or
  `--tax-slab 30`, `--max-stock-weight 10`, ...). `stockwise profile reset`
  goes back to the defaults.

## Broker secrets and the keyring

Broker secrets go into the OS keyring when one is available. `doctor` shows
one of:

```
  Keyring                      ✓ macOS Keychain available for broker secrets
  Keyring                      not used (...); broker secrets stay in owner-only files
```

The second is not an error: the secrets stay in the broker config file, readable
only by you. On Linux without a running Secret Service (a server, a container,
WSL) the keyring is not available. `stockwise trade config --keyring` moves
secrets into the keyring once one exists, and fails with `No OS keyring is
available here (...)` when it does not. `STOCKWISE_KEYRING=off` keeps secrets
in the file on purpose.

## Notifications do not arrive

`stockwise notify status` lists each channel (Telegram, Slack, Discord, email,
ntfy, Pushover, desktop) as configured or not, and the variables that turn one
on. `doctor` checks the same channels. Then:

- `stockwise notify test` sends a test message to every configured channel and
  prints the error from each one that fails.
- Quiet hours (`STOCKWISE_QUIET_HOURS`) hold all but high-priority notices, and
  routes (`STOCKWISE_NOTIFY_ROUTES`) set the lowest priority per channel.
- The worker started as a login service does not see variables from your shell.
  Put the notification settings in the config file (see below).
- Desktop pop-ups need a build with the `desktop-notify` feature; release
  binaries do not have it.

## The background worker is not running

Alerts, the holding monitor, the after-close digest and post-close notices need
the worker. Check it:

```bash
stockwise worker status          # running or not, tasks queued, last end-of-day sync
stockwise worker service-status  # unit file, whether launchd / systemd loaded it, worker running
```

`doctor` checks the login service too. The usual fixes:

- Not installed: `stockwise worker install` (preview with `--dry-run`), or
  `stockwise worker start` for this session only.
- Installed but not loaded: run `stockwise worker install` again; it replaces
  the unit and restarts it.
- It starts and stops: read the log. macOS: `worker.log` in the data directory.
  Linux: `journalctl --user -u stockwise-worker`.
- A worker started with `worker start` holds the lock: `stockwise worker stop`
  first, then the service's worker takes over.
- Linux stops user services at logout unless lingering is on:
  `loginctl enable-linger`.
- The unit copies `STOCKWISE_HOME`, `STOCKWISE_CONFIG` and `STOCKWISE_PORTFOLIO`
  only. Other variables, such as notification secrets, belong in the config
  file.

## The AI app does not see StockWise's tools

`stockwise connect` writes the MCP entry into each AI app's config. If the app
shows no StockWise tools:

1. `stockwise connect --list` shows each app, whether it is found and where its
   config lives. `doctor` checks the same configs.
2. Restart the app. Apps read their MCP config at start.
3. Check that the entry runs: `stockwise mcp --list-tools` prints the tools as
   JSON. If that fails, so does the app.
4. The app may not find `stockwise` on its `PATH` (GUI apps on macOS do not read
   your shell profile). `connect` writes the full path of the binary it runs
   from; run `stockwise connect APP` again after you move or reinstall it.
5. For Claude Code: `claude mcp list` should show `stockwise`; add it with
   `claude mcp add stockwise -- stockwise mcp`.
6. Write tools (watchlist, alerts, goals) appear only with
   `stockwise connect --allow-writes`. StockWise never places orders from an
   assistant.

More in [Use StockWise from AI assistants](guide-mcp.md) and [ai.md](ai.md).

## AI answers fail or are rate limited

`ask`, `report` and `--ai` use a local Ollama model unless
`STOCKWISE_AI_PROVIDER` picks a cloud one. `doctor` shows the state:

```
  AI                           Ollama not running (optional; needed for `report` and --ai)
  AI                           ! Ollama is running, but model ... is not pulled — run `ollama pull ...`
  AI                           ! <provider> selected but <KEY> is not set
  AI                           ✗ unknown provider '...' — use ollama, openai, anthropic or gemini
```

A cloud provider that refuses a request gives `<provider> returned HTTP 429:
...` (rate limit or quota) or another status with the provider's own message.
Wait and retry, lower `ask --max-steps` or `--max-tokens` so one question
makes fewer calls, or switch to a local model. `Could not reach <provider>`
means the network request failed. See [ai.md](ai.md).

## Still stuck?

Run the command with `--verbose` to see its network requests, or with `--json`
(errors go to stderr as JSON, so stdout stays clean), or check the
[commands reference](commands.md). To report a bug, include the command, its
full output, `stockwise --version` and `stockwise doctor --json` in an
[issue](https://github.com/iamsaquib8/stockwise/issues/new/choose). Do not paste
files from your data folder, broker config or API keys.
