# Terminal dashboard (`stockwise tui`)

A full-screen, live version of `stockwise dashboard`: market indices, your
watchlist, portfolio P&L, price alerts, and a chart of the selected symbol,
refreshed in place.

![stockwise tui](../assets/tui.svg)

```bash
stockwise tui              # Indian market (default)
stockwise tui -m us        # US indices; bare tickers are US symbols
stockwise tui -i 60        # refresh every 60 seconds (default 30, range 10-3600)
```

**Read-only.** The TUI never places orders and has no trading keys. The only
thing it changes is your local watchlist (add and remove). Live trading stays
in `stockwise trade`, with its preview and confirmation.

## Layout

| Pane | Shows |
|------|-------|
| Header | Market, time of the last refresh, ⟳ while a refresh runs |
| Indices | NIFTY 50, SENSEX, BANK NIFTY, NIFTY IT (or the US indices with `-m us`). Prices are dropped on narrow terminals |
| Watchlist | Price and day change for every watched symbol |
| Chart | Closing prices of the selected symbol for the selected period, with the change over the period |
| Portfolio | Total value and P&L, then each holding. Holdings without a live price show N/A and are left out of the totals (the title says how many) |
| Alerts | Triggered alerts first (marked `!`), then the rest, with the live price. An alert never fires without a price |
| Footer | Key hints, or the latest message (errors in red) |

Press `p` for the **portfolio analysis** screen: holdings with weight, P&L,
today's move, tax term (`LT`, `ST 17d`, `mix`), long-term score and
observation marks (`●` a notice, `○` information) on the left; summary,
allocation by market and sector, risk and concentration, and the observations
on the right. `Enter` opens a holding in detail: market, currency, cap tier,
sector, value in its own and the base currency, gains by tax term, the next
lot to turn long-term, volatility, beta, score and its observations. It is
the same analysis as `stockwise portfolio analyze`, loaded when you first
open the screen; `r` runs it again.

The chart follows the selection: move through the watchlist or, after `Tab`,
the portfolio. With both empty it shows the first index.

## Keys

| Key | Action |
|-----|--------|
| `↑` `↓` / `j` `k` | Move the selection (the chart follows) |
| `g` `G` / `Home` `End` | First / last row |
| `Tab` | Switch between watchlist and portfolio |
| `d` / `Enter` | Inspect the selected symbol: live price, day volume, your position and its weight, active alerts (`Esc`, `d` or `Enter` closes it) |
| `s` | Cycle the sort order of the watchlist and portfolio: file order, symbol A–Z, change % (day change for the watchlist, P&L for holdings), value (price for the watchlist, value in the base currency for holdings). The pane titles show the active sort. The selection stays on the same symbol when the rows move |
| `p` | Open or close the portfolio analysis screen (`Esc` also closes it) |
| `1`-`6` / `[` `]` | Chart period: `1d` `5d` `1mo` `6mo` `1y` `5y` |
| `/` or `a` | Search a company or ticker and add it to the watchlist (`Enter` searches, `↑` `↓` choose, `Enter` adds, `Esc` cancels). Listings on your market's exchanges (NSE/BSE for India) are listed first |
| `x` / `Del` | Remove the selected watchlist symbol |
| `r` | Refresh now |
| `?` | Show or hide the help |
| `q` / `Ctrl+C` | Quit |

## Refresh and rate limits

Every refresh is one batched quote request for the indices, watchlist,
holdings and alert symbols together, plus one chart request. All requests go
through the same client as the CLI, so they share its rate limiter (5
requests/second), retries, and cache. Quotes are cached for 30 seconds (in
Redis too, when it runs), so an interval below 30s mostly re-reads the cache.

If a refresh fails (network down, Yahoo rate-limiting), the last data stays on
screen, the footer shows the error, and the next refresh tries again.

## Terminal support

- Works in any terminal crossterm supports (macOS, Linux, Windows Terminal).
- **80×24** shows everything. Smaller (down to 40×12) drops the chart and
  shows the alerts as one line; smaller still shows a "too small" message
  until you resize.
- `NO_COLOR=1` turns colours off; direction is still shown by ▲/▼ and +/−
  signs, and the selection by reverse video.
- The terminal is restored on `q`, `Ctrl+C`, `SIGINT`, errors, and panics.
- Needs an interactive terminal. For a one-shot view use `stockwise dashboard`;
  for scripts, the `--json` commands or [`stockwise serve`](server.md).

## Screenshots

`stockwise tui --snapshot 100x30` fetches the data once, prints a single
frame, and exits. With `CLICOLOR_FORCE=1` the frame keeps its colours when
piped, which is how `assets/tui.svg` is made:

```bash
CLICOLOR_FORCE=1 stockwise tui --snapshot 100x30 \
  | python3 assets/ansi2svg.py assets/tui.svg "stockwise tui"
```

## Building without it

The TUI is the `tui` cargo feature, on by default. A minimal build leaves out
ratatui and crossterm:

```bash
cargo install stockwise --no-default-features            # no tui, no serve
cargo install stockwise --no-default-features --features tui
```

`stockwise tui` in a build without the feature exits with a message saying so.
