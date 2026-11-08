# Data providers

StockWise gets market data from **Yahoo Finance** by default — no account or
key needed. You can opt in to another provider for quotes, daily history and
symbol search. Yahoo stays in the loop either way: it fills every field the
other provider lacks and answers whatever the provider can't.

```bash
export STOCKWISE_DATA_PROVIDER=twelvedata
export TWELVEDATA_API_KEY=...
stockwise doctor            # "Twelve Data answered for AAPL (180 ms)"
stockwise quote AAPL -m us  # ends with "Data: Twelve Data"
```

## Providers

| Provider | `STOCKWISE_DATA_PROVIDER` | Key variable | Free tier | Covers on the free tier |
|----------|---------------------------|--------------|-----------|-------------------------|
| Yahoo Finance | unset or `yahoo` | — | unofficial, no key | Everything StockWise does |
| Alpha Vantage | `alphavantage` | `ALPHAVANTAGE_API_KEY` | 25 requests/day; end-of-day; last 100 daily bars | US stocks, BSE (`.BO`), LSE, TSX, XETRA, Shanghai/Shenzhen, FX, crypto |
| Massive (formerly Polygon.io) | `massive` or `polygon` | `MASSIVE_API_KEY` (or `POLYGON_API_KEY`) | 5 requests/min; end-of-day; 2 years of history | US stocks, FX, crypto |
| Twelve Data | `twelvedata` | `TWELVEDATA_API_KEY` | 8 credits/min, 800/day | US stocks, FX, crypto (NSE/BSE need the paid Grow plan) |
| NSE website | `nse` | — | unofficial; ≤3 requests/s | NSE stocks (`.NS`) and NSE indices (`^NSEI`, `^NSEBANK`, ...) — see [NSE and BSE](#nse-and-bse) |
| BSE website | `bse` | — | unofficial; ≤3 requests/s | BSE stocks (`RELIANCE.BO`, `500325.BO`) |
| NSE + BSE | `exchange` | — | as above | `.NS` and NSE indices from NSE, `.BO` from BSE, everything else from Yahoo |

Free-tier limits are the providers' own and change over time; check their
pricing pages. Twelve Data's free plan is licensed for internal, non-display
use only.

**Indian stocks:** none of the keyed free tiers carries NSE, and only Alpha
Vantage carries BSE. With those providers, `TCS` (`TCS.NS`) keeps coming from
Yahoo; `RELIANCE.BO` goes to Alpha Vantage. For prices straight from the
exchanges, use `nse`, `bse` or `exchange` below. SENSEX (`^BSESN`) and
non-Indian indices always come from Yahoo.

## What goes where

| Data | With a provider configured |
|------|----------------------------|
| Quotes (`quote`, `dashboard`, `portfolio`, ...) | Provider for price, day range, volume and change; Yahoo for name, 52-week range, fundamentals and anything else missing. `quote` says which provider answered |
| Bulk scans (`movers`, `screen`, `heatmap`, ...) | Yahoo, when the scan has more symbols than the provider's per-run budget, so one table never mixes sources |
| Daily, weekly, monthly history | Provider, if its plan reaches back over the whole period (weekly and monthly bars are built from daily ones); otherwise Yahoo. Ranges beyond the free plan — over ~140 days on Alpha Vantage, over 2 years on Massive, and `-p max` on both — go straight to Yahoo without spending a request |
| Intraday bars (`-p 1d`, `-p 5d`) | Yahoo (not on any free tier) |
| `search` | Provider's matches, mapped back to Yahoo symbols (`RELIANCE.BSE` → `RELIANCE.BO`, `X:BTCUSD` → `BTC-USD`); Yahoo's when it finds none |
| Fundamentals, news, insider, earnings | Yahoo |

## NSE and BSE

```bash
export STOCKWISE_DATA_PROVIDER=exchange   # or nse, or bse; or data_provider = "exchange" in config.toml
stockwise doctor              # "NSE answered for RELIANCE.NS (640 ms)", "BSE answered for 500325.BO (...)"
stockwise quote RELIANCE      # ... Circuit Limits ₹1,307.20 — ₹1,597.60, Delivery 52.35%, "Data: NSE"
stockwise quote 500325        # a bare 6-digit number is a BSE scrip code (500325.BO)
stockwise movers --universe nifty50
```

**What these are.** NSE and BSE have no free public market-data API. Their
websites load prices from JSON endpoints meant for the site's own pages, and
these providers read the same endpoints. They are **unofficial and
undocumented**: the exchanges can change, rate-limit or block them at any time,
and their terms of use restrict automated access and redistribution of their
data. Use them for your own analysis; don't republish what they return, and
check NSE's and BSE's terms of use yourself. Licensed real-time feeds come from
the exchanges' data vendors (NSE Data & Analytics, BSE's data products) and
from brokers' market-data APIs.

**What StockWise reads**

| | NSE (`nse`) | BSE (`bse`) |
|---|---|---|
| Quote | LTP, change, open, day high/low, previous close, 52-week range, upper/lower circuit; for up to three symbols at a time also volume, market cap and **delivery %** | LTP, change, open, day high/low, previous close |
| Indices | `^NSEI` (NIFTY 50), `^NSEBANK`, `^CNXIT`, `^NSMIDCP` (NEXT 50), `^CNX100`, `^CNX200`, `^CRSLDX` (500), `^NSEMDCP50`, sector indices (`^CNXPHARMA`, `^CNXAUTO`, ...), `^INDIAVIX` — all in one request | — (SENSEX stays on Yahoo) |
| Intraday bars (`-p 1d`) | Today's session, 1m–90m bars built from NSE's price chart, aligned to 09:15 IST; **no volume** (NSE's chart has prices only) | Today's session from BSE's price chart; no volume |
| Daily / weekly / monthly bars | Up to 5 years (one request per year of history) | Up to 5 years (one CSV download) |
| Search | NSE's autocomplete; NSE's listed-equity file (`EQUITY_L.csv`, kept a day) if that fails | BSE's scrip list (kept on disk for a day): ids, scrip codes and company names |
| Symbols | `RELIANCE.NS`, `M&M.NS`, NSE indices | `RELIANCE.BO` (looked up in the scrip list) or `500325.BO` |

Everything else — names when the exchange gives none, fundamentals,
multi-day intraday bars (`-p 5d`), history beyond 5 years or `-p max`,
dividends and splits — comes from Yahoo, exactly as with the other providers.
`--json` quotes from NSE add `deliveryPercent`, `lowerCircuit` and
`upperCircuit` (and `dataSource: "NSE"`); `--csv` has `delivery_percent`,
`lower_circuit` and `upper_circuit` columns, empty for other sources.

**Being a polite client**

- One session per site per run: NSE's home page is loaded once for the
  cookies its API expects; an expired session (HTTP 401) is renewed once.
- At most three requests a second per site, with random jitter on top, across
  everything a command does.
- Timeouts, HTTP 429 (honouring `Retry-After`, capped at 10 s) and 5xx are
  retried twice with exponential backoff.
- **A refusal is final.** HTTP 403 or an "Access Denied" page means the site
  doesn't want automated requests right now: StockWise prints one `ⓘ` notice
  on stderr, stops asking that site for 15 minutes (the rest of a normal run),
  and Yahoo answers. It never retries a refusal with different headers.
- If an endpoint disappears (404) or answers in a shape StockWise doesn't
  know, it says *"NSE changed its response format (...)"* once and Yahoo
  answers. `doctor` shows the same reason.
- Answers share the Redis / in-memory cache (quotes 30 s, intraday 1 minute,
  daily history 10 minutes, search 1 hour); the BSE scrip list and NSE's
  equity file are kept in the data directory's `cache/` for a day.
- Big scans stay on Yahoo: a quote request for more than 12 uncached NSE
  symbols (8 on BSE) goes to Yahoo in one request rather than waiting on the
  rate limit. Members of an index fetched with `--universe` are already priced
  by that one NSE request, so `movers --universe nifty500` costs one NSE
  request and no Yahoo request, and still works when Yahoo is unreachable;
  `screen --universe` also asks Yahoo for P/E and market cap.

**Mixed mode** (`exchange`): `.NS` symbols and NSE indices go to NSE, `.BO`
symbols to BSE, and US stocks, crypto, FX and SENSEX to Yahoo — so a
portfolio holding both `TCS` and `AAPL` gets each price from its own market.
`search` lists NSE matches first, then BSE's.

**Index members** (`--universe` on `movers`, `heatmap` and `screen`) come from
NSE whichever provider is set; see [commands.md](commands.md#market). Offline,
the built-in lists stand in.

The fields StockWise reads from each endpoint, and how to record a fresh
response for a test fixture, are in
[exchange-responses.md](exchange-responses.md).

## Limits and fallback

- Each command spends at most a small allowance of provider requests —
  4 for Alpha Vantage, 5 for Massive, 8 for Twelve Data — so one scan can't burn
  a day's allowance or wait a minute for the next slot. The allowance comes
  back after a window: a minute for Massive and Twelve Data, 4 h 48 min for
  Alpha Vantage (4 a window: two windows can touch, so 5 could reach 29 in a
  day against its limit of 25). A long-running `serve`, `tui`,
  `mcp` or daemon therefore keeps using the provider instead of dropping to
  Yahoo after the first few requests.
- If a provider reports a rate limit, refuses the key, or can't be reached, StockWise prints one
  `ⓘ` notice on stderr, stops asking it for a while (to the end of the window
  after a rate limit, 3 minutes after a network error, an hour after a refused
  key) and uses Yahoo meanwhile. A later pause prints its own notice.
  `--json` output on stdout is unaffected.
- Yahoo's cookie-and-crumb handshake, which quotes, fundamentals and options
  need, is tried once per call. When it fails, the failure is remembered for
  45 seconds and later calls get the same error at once, so a command that makes
  dozens of fundamentals calls doesn't repeat six requests and their waits for
  each. Charts and search need no crumb and are unaffected.
- When AMFI's NAV file or a scheme's NAV history can't be downloaded, `mf`
  commands use the saved copy only if it is at most 7 days old; `mf nav` and
  `mf history` then say so under the table and add a `stale` entry (source,
  reason, day saved) to `--json`. An older copy is an error that says how old
  it is.
- A provider's quote is dated by its own trading day (Alpha Vantage's "latest
  trading day", Twelve Data's timestamp, the day of Massive's latest bar), so
  an end-of-day quote from yesterday is never taken for today's trade.
- Provider answers share the Redis / in-memory cache (quotes 1–5 minutes,
  daily history 10 minutes), so repeating a command doesn't spend another request.
  API keys are never part of a cache key or an error message.
- An unknown provider name or a missing key is reported (by every command on
  stderr, and by `doctor`) and StockWise carries on with Yahoo.
- `doctor` asks the provider for one `AAPL` quote to show it answers — one
  request of your allowance per run. With `nse`, `bse` or `exchange` it asks
  for `RELIANCE.NS` and/or `500325.BO` instead and reports each exchange on
  its own line.

`--json` quotes from another provider carry a `"dataSource"` field naming it;
Yahoo-only quotes don't have the field.

## Adding a provider

Implement the `MarketData` trait in `src/providers/` (quote, daily series,
search), map Yahoo-style symbols to the provider's syntax and back, and add a
`ProviderKind`. Parse responses in pure functions with fixture tests, and test
the HTTP path against `testutil::MockServer` — tests must never call a real
API or need a real key. A source without an API (a website's JSON) goes
through `providers::session::Session`, which brings the rate limit, retries,
caching and refusal handling.
