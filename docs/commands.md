# Commands Reference

`stockwise --help` lists the core commands; `stockwise --help-all` lists every
command by group. `stockwise <command> --help` shows a command's arguments.
This page adds detail.

Looking for a walkthrough instead? [What do you want to do?](guides.md) lists
task-based guides, and each section below links to its guide.

## Global options

| Option | Env var | Description |
|--------|---------|-------------|
| `-m, --market <MARKET>` | `STOCKWISE_MARKET` | Market for bare tickers. Default `in`, or the market saved by `stockwise use` (see [Market and portfolio context](#market-and-portfolio-context)): `TCS` → `TCS.NS`. `us`: `AAPL`. `crypto`: `BTC` → `BTC-USD`; `BTC/USD`, `BTC_USD` and `BTC USD` also resolve to `BTC-USD`. `fx` (or `forex`): `USDINR`, `usd/inr` → `USDINR=X`. `commodity`: `GOLD` → `GC=F`, `SILVER` → `SI=F`, other bare tickers → NSE (`GOLDBEES` → `GOLDBEES.NS`). Global exchanges from the [market table](markets.md): `uk` (`VOD` → `VOD.L`), `de` (`SAP.DE`), `jp` (`7203` → `7203.T`), `hk` (`5` → `0005.HK`), `sg` (`D05.SI`), `ca` (`RY.TO`), `au` (`BHP.AX`). Symbols with a suffix (`RELIANCE.BO`, `VOD.L`), indices (`^NSEI`), FX (`USDINR=X`), futures (`GC=F`) and crypto pairs (`BTC-INR`) pass through unchanged |
| `--quote-currency <CODE>` | `STOCKWISE_QUOTE_CURRENCY` | Currency bare crypto tickers are quoted in with `-m crypto`. Default `usd`; `inr` gives `BTC-INR` |
| `--lang <en\|hi>` | `STOCKWISE_LANG` | Language for help groups, tables and messages. Default: your locale (`LC_ALL` / `LC_MESSAGES` / `LANG` starting with `hi` picks Hindi), else English. `--json` and `--csv` output is never translated |
| `--ai` | `STOCKWISE_AI` | Add an AI section to `analyze`, `technical`, `compare`, `screen`, `backtest`, `deep` (slow with a local model; see [ai.md](ai.md)) |
| `--no-ai` | | Turn the AI section off for one run, even if `STOCKWISE_AI` or the config file turn it on |
| `--portfolio <NAME>` | `STOCKWISE_PORTFOLIO` | Use a named portfolio (`portfolios/NAME.json`) instead of the default `portfolio.json`. Applies to `portfolio`, `import`, `tax`, `wealth`, `dashboard`, `trade holdings` and every other command that reads holdings |
| `--profile <MODE[:RISK]>` | `STOCKWISE_PROFILE` | Investor profile mode (`passive`, `active`, `active:low`, `active:med`, `active:high`, `ultra`) and personal guardrails |
| `--verbose` | `STOCKWISE_LOG` | Network diagnostics on stderr: each request's host, status, time and whether the cache answered. `STOCKWISE_LOG=debug\|info\|warn` gives the same output with finer control. Standard output is unchanged, so `--json` stays parseable. See [troubleshooting.md](troubleshooting.md) |
| `--help-all` | | List every command by group (`--help` lists the core commands) |
| `-V, --version` | | Print the version |

Other environment variables: `STOCKWISE_HOME` (data directory),
`STOCKWISE_REDIS_URL` (cache server, or `off`), `STOCKWISE_DATA_PROVIDER`
and its API key ([data-providers.md](data-providers.md)),
`STOCKWISE_AI_PROVIDER`, `STOCKWISE_MODEL`, `OLLAMA_HOST` and the cloud AI keys
([ai.md](ai.md)), `STOCKWISE_CONFIG` (config file path), `STOCKWISE_BIND` and
`STOCKWISE_API_TOKEN` (`serve`), and the notification variables
([bots.md](bots.md#notifications)). `NO_COLOR` disables colour; colour is also
off when output is piped.

Defaults for most of these, and for each command's `-p`, can live in a
[config file](#configuration-file). Precedence: command-line flag >
environment variable > config file > built-in default. API keys and the server
token are read from the environment only.

**Crypto and FX**: prices show with the precision they need (FX pairs 4
decimals, coins under 1 up to 8) and `quote` notes trading hours (crypto 24/7;
FX 24 hours Monday–Friday, closed at weekends). Commands built on exchange
sessions, sectors or fundamentals — `sectors`, `rotation`, `sectorcmp`,
`screen`, `picks`, `matrix`, `surprise`, `peers`, `intraday`, `longterm`,
`daemon`, `sim start` — refuse `-m crypto` / `-m fx` with an explanation;
`movers`, `heatmap`, `history`, `technical`, `backtest` and friends work.

**Global exchanges** (`-m uk|de|jp|hk|sg|ca|au`): quotes, history, charts,
technicals, `movers` (each exchange's large caps), `markets` and alerts work as
for India and the US. Prices show in the local currency (`£`, `€`, `¥`, `HK$`,
`S$`, `C$`, `A$`); London quotes in pence, so `VOD.L` shows `118.20p` while its
market cap shows in pounds. `sectors`, `rotation` and `sectorcmp` need sector
indices or ETFs, which Yahoo has for Japan (TOPIX-17 ETFs), Hong Kong, Canada
and Australia only. Sessions, lunch breaks, half days and holidays come from
each exchange's calendar ([markets.md](markets.md)). `tax` has rules for India,
the US and the UK only, and names the other listings it leaves out. Backtest
costs default to the US model outside India. The bots and the simulator
(`intraday`, `longterm`, `daemon`, `sim start`, `sim replay`, `sim challenge`,
`report`) size positions from the capital you type, so they refuse `-m uk`,
whose prices are in pence; replays on other exchanges use their main index
as the benchmark.

**Commodities** (`-m commodity`): `GOLD`/`SILVER` resolve to COMEX futures, so
`history gold`, `chart silver`, `technical gold` and `alert add gold above
4200` work; `commodity` and `bonds` have their own commands
([below](#commodities--bonds)).

**Hindi** (`--lang hi`): help group titles and the main command descriptions,
`quote`, `markets`, `dashboard`, `portfolio`, `search` and `doctor` labels,
and common errors. Other commands are still English. Indian amounts keep
lakh/crore grouping (`₹16.27 लाख करोड़`).

**Periods** (`-p`): `1d`, `5d`, `1w` (same as `5d`), `1mo`, `3mo`, `6mo`, `ytd`, `1y`, `2y`, `5y`, `10y`, `max`.
Value names (`screen growth`, `tax in`, `-s macd`) are case-insensitive.

**Exit codes**: `0` success, `1` runtime error (unknown symbol, no data,
network, invalid config file), `2` invalid arguments.

**Errors with `--json`**: the error prints as one JSON object on stderr,
`{"error":{"message":"...","exit_code":N}}`, with the same exit code. Standard
output stays empty, so a script can tell an error from a result.

**Machine-readable output**: commands marked `[--json]` print JSON, and
`[--json\|--csv]` print JSON or CSV, instead of a table. See
[Scripting](#scripting-with---json-and---csv).

## Start here

> **Guide:** [Quickstart: five minutes](quickstart.md) walks through the first commands.

`stockwise SYMBOL` with no command prints a one-screen overview that starts with the
stock's [action](#actions), and bare `stockwise` prints a quick start. Short names: `q` (quote), `d` (dashboard), `a` (analyze), `t` (technical),
`c` (compare), `p` (portfolio), `w` (watch), `ev` (events). The aliases are lowercase: a capital
single letter is a ticker, so `stockwise C -m us` (Citigroup), `T`, `D`, `A` and `W` work as bare
symbols, and `stockwise c TCS INFY` still compares. A longer ticker that spells a command, such as
`ALL` or `PATH`, is a symbol when you give `-m` (`stockwise PATH -m us`). `stockwise quote C -m us`
always works. Common names resolve to NSE symbols
(`SBI` → `SBIN`, `L&T` → `LT`, index names), and `quote` suggests close matches for a mistyped symbol.

Without `-m`, a symbol that says where it trades is looked up there: see
[Market and portfolio context](#market-and-portfolio-context).

| Command | Description |
|---------|-------------|
| `quote <SYMBOLS...> [--json\|--csv]` (`q`) | Price, change, day/52-week range, volume, market cap, P/E, EPS, moving averages. Unknown symbols are named in the error |
| `deep <SYMBOL> [-p PERIOD]` | Everything about one stock, starting with its [action](#actions): fundamentals, technicals, support/resistance, Fibonacci, risk, gaps, volume, insights. Default `3mo` |
| `markets [--crypto] [--fx] [--global] [--json\|--csv]` | US + India indices (S&P 500, Dow, NASDAQ, Russell, VIX, NIFTY 50, SENSEX, Bank NIFTY, NIFTY IT) and the India 10-year G-sec yield (CCIL, RBI fallback; left out when neither answers); `--crypto` adds the largest coins, `--fx` major pairs (USD/INR, EUR/USD, ...), `--global` one row per exchange's main index (NIFTY 50, S&P 500, FTSE 100, DAX, Nikkei 225, Hang Seng, Straits Times, S&P/TSX, S&P/ASX 200) with whether it is open and its local time. With `-m crypto`, `-m fx`, `-m commodity` or a global exchange that section comes first. `--json` rows carry `region`: `Indian Markets`, `US Markets`, `Crypto`, `FX`, `Commodities`, a country (`United Kingdom`, `Japan`, ...) or `Global` |
| `dashboard` (`d`) | Indices + portfolio value, P&L and day change (value and %, against each holding's previous close) + watchlist + triggered alerts; first-run hints when empty |
| `today [--days N] [--no-news] [--limit N] [--json]` | What changed in your portfolio today: each holding's day move and P&L with why it moved (the most important fresh headline or exchange/SEC filing, "with the market", or "unexplained" for a move of 2.5σ or more against the benchmark with no news found), σ and volume multiple, dates in the next 3 days; the watchlist; the market backdrop; and one news feed for holdings and watchlist ranked by portfolio weight × importance (`--days`, default 2). See [What's hot and today](#whats-hot-and-today) |
| `hot [--cap TIER] [--universe INDEX] [--limit N] [--json]` | What's hot: your holdings and watchlist ranked by heat, then the market (biggest moves, unusual volume, gaps, 52-week highs/lows, most-mentioned in news, sectors, theme baskets, FII/DII flows, bulk/block deals, F&O ban list, OI build-up, cross-market movers). India and US. See [What's hot and today](#whats-hot-and-today) |
| `search <QUERY> [--json\|--csv]` | Find a ticker by company name |
| `doctor [--json]` | Check the config file, market and language, data directory, Yahoo reachability, the data provider that answered, Redis, AI provider and model (never the key), broker config, notification channels, the worker login service (launchd / systemd), the MCP config of each AI app, the tax residence and investor profile, and whether a newer StockWise release exists (checked at most once a week; the answer is cached in `update-check.json` in the data directory). Each failed check prints a fix hint. `--json` prints the checks as an array. Exits 1 if market data is unreachable or the config file is invalid. See [troubleshooting.md](troubleshooting.md) |
| `cache status` | Entries, size and location of `cache.db` |
| `cache clear [http\|quotes\|snapshots\|bars\|results\|members\|bhavcopy\|all]` | Delete cached data (default `http`). Your portfolio and settings are untouched |
| `cache warm [--background]` | Fill `market_snapshots` in `cache.db` with the market's universe and your holdings. `--background` queues it for the worker |
| `cache snapshots [--cap TIER] [--sector NAME] [--min-pe N] [--max-pe N] [--min-roe PCT] [--min-div-yield PCT] [--min-rvol X] [--sort FIELD] [--asc] [--limit 25] [--json]` | Filter the cached snapshots offline. Sort by `symbol`, `mcap` (default), `price`, `change_1d`, `rvol`, `pe`, `roe` or `div_yield`, largest first unless `--asc` |
| `tasks` / `tasks list [--limit 30] [--json]` | Recent background tasks and their progress |
| `tasks show <ID> [--json]` / `tasks cancel <ID>` | One task with its result / cancel a queued or running task |
| `tasks add <warm\|eod\|run> [-- ARGS...]` | Queue a task for the worker: `warm` the cache, the post-close `eod` notices, or `run` any stockwise command (`tasks add run -- backtest TCS`) |
| `init [--import SOURCE:FILE]... [--no-mcp]` | First run: pick a market, import holdings (`--import groww:holdings.csv`, repeatable; sources `kite`, `groww`, `upstox`, `indmoney`, `cas`) and optionally connect an AI app (`--no-mcp` skips that step) |
| `use [MARKET\|PORTFOLIO]... [--bind\|--unbind] [--json]` | Show the market and portfolio in use, or switch them: `use us`, `use long-term`, `use us-tech us --bind`. See [Market and portfolio context](#market-and-portfolio-context) |

### Background worker

`stockwise worker start` runs the background worker (live quotes while the
market is open, queued tasks, alerts, the post-close sync) detached until
`worker stop` or a reboot. To have it start at every login instead:

| Command | Description |
|---------|-------------|
| `worker start` / `worker stop` / `worker status` / `worker run` | Start it detached / stop it / show whether it runs and what is queued / run it in this terminal |
| `worker install [--dry-run] [--interval MIN]` | Run the worker at login as a per-user service: a launchd agent on macOS (`~/Library/LaunchAgents/com.stockwise.worker.plist`, output in `worker.log` in the data directory), a systemd user service on Linux (`~/.config/systemd/user/stockwise-worker.service`, output in `journalctl --user -u stockwise-worker`). It runs this executable as `stockwise -m MARKET worker run` and is restarted `--interval` minutes (default 1) after it exits with an error, never after a clean `worker stop`. Reinstalling replaces the unit and restarts it. `--dry-run` prints the unit file and the commands and changes nothing. Other systems get an error: start the worker yourself at login |
| `worker uninstall [--dry-run]` | Stop the service and remove its unit file |
| `worker service-status` | Whether the unit file exists, whether launchd / systemd has it loaded, and whether a worker is running |

Only user-level locations are written; nothing needs `sudo`. `STOCKWISE_HOME`,
`STOCKWISE_CONFIG` and `STOCKWISE_PORTFOLIO` are copied into the unit when set,
so the service uses the same data directory, config file and portfolio. Other
variables, notification secrets in particular, are not: the unit is a
plain-text file, so keep those in the config file. The commands it runs:

```text
# macOS
launchctl bootout gui/$UID/com.stockwise.worker        # if loaded; errors ignored
launchctl bootstrap gui/$UID ~/Library/LaunchAgents/com.stockwise.worker.plist
                                                       # or `launchctl load -w` on old macOS
# Linux
systemctl --user daemon-reload
systemctl --user enable --now stockwise-worker.service
```

A systemd user service stops when you log out unless lingering is on
(`loginctl enable-linger`). If a worker started with `worker start` is
already running, stop it first: the service's worker cannot take the lock
while it runs and retries after `--interval`.

## Analysis

> **Guide:** [Check a stock or fund fast](guide-research.md).

| Command | Description |
|---------|-------------|
| `analyze <SYMBOL>` (`a`) | Valuation, profitability, growth, analyst ratings, plain-English outlook |
| `results <SYMBOL> [--quarters N] [--annual] [--json\|--csv]` | Last 8+ quarters side by side: revenue, operating profit, OPM %, other income, interest, depreciation, PBT, tax %, PAT, diluted EPS, YoY %, QoQ %, OPM change in bps, a TTM column, and EPS against consensus with the 1-day and 5-day move versus the index. Banks add NII and GNPA/NNPA. `--annual` shows fiscal years with CFO/PAT, FCF, ROE, ROCE and 3/5/10-year CAGR. Needs no key; results are kept in `cache.db` for six hours |
| `results --holdings` / `--universe nifty50` `[--sort date\|pat\|margin\|surprise]` | Earnings-season scoreboard: how many companies reported the active quarter, aggregate revenue, operating profit and PAT growth YoY, beat / in-line / miss counts, and the reporters table |
| `events <SYMBOL> [--days N] [--ai]` (`ev`) | Unified timeline of material corporate events, exchange filings, earnings results, and excess market reaction vs NIFTY/S&P 500 |
| `insights <SYMBOL>` | Positive, negative and neutral findings (Growth, Valuation, Margins, Ownership, etc.) with quantified metrics and XBRL shareholding patterns |
| `verdict <SYMBOL> [--horizon short\|long] [--explain] [--json]` | Starts with the stock's [action](#actions) for the horizon (default `long`), then the 6-pillar score: valuation (20%), growth & profitability (25%), balance sheet (15%), smart money & ownership (15%), analyst consensus (10%) and technical trend (15%). Each pillar gets 0–100 and a Strong / Moderate / Weak band, or "no data" when its inputs are missing; the composite (0–100) leaves "no data" pillars out and `data_coverage` says how much of the input was present. Crypto, FX and indices get only the technical pillar. Size cut-offs (cash, insider trades; market cap in `longterm`) are written in US dollars and amounts in other currencies are converted at rough fixed rates, so the same size scores the same in rupees and dollars. `--explain` lists the weights and rules |
| `score <SYMBOL> [--json] [--explain]` (`scorecard`, `fundamental score`) | Transparent accounting scorecards. **Piotroski F-Score** (2000): nine yes/no signals, most of them this year against last year. A signal whose data is missing shows `n/a`, earns no point and is not counted, so the score reads like `5/7 evaluated`; it is rated only when all nine ran. Net income is estimated from trailing EPS × shares, or margin × revenue, only when no period reports it, and is marked `(est.)`. **Altman Z** (1968): needs all five ratios; with any missing it shows `n/a` and names them, with no zone, and no input is guessed. **Altman Z''** (1995, non-manufacturers) uses book equity over liabilities and is shown with its own factor table whenever X1–X4 exist. In `--json`: `piotroski.evaluated` and `criteria[].evaluated` (`piotroski.rating` is null when not rated), `altman.missing_factors`, `altman.z_double_prime_components` and `altman.z_double_prime_formula_str` |
| `technical <SYMBOL> [-p] [--json] [--explain]` (`t`) | RSI, MACD, Bollinger Bands, ATR, VWAP, SMA/EMA, braille charts. Default `3mo` |
| `history <SYMBOL> [-p] [--json\|--csv]` | Braille price chart, volume bars, last 20 bars. `--json`/`--csv` emit every OHLCV bar. Default `1mo` |
| `chart <SYMBOL>` | 1W, 1M, 3M and 1Y charts at once |
| `compare <SYMBOLS...> [--json\|--csv]` (`c`) | Two or more stocks side by side across 16 metrics |
| `risk <SYMBOL> [-p] [--json]` | Annualized volatility, Sharpe/Sortino/Calmar, max drawdown, VaR 95/99% — always from daily bars, annualized with 252 bars a year for stocks and indices, 365 for crypto and 260 for FX. Default `1y` |
| `correlate <SYMBOLS...> [--json\|--csv]` (`corr`) | Correlation matrix of daily returns (two or more symbols), from the days both traded, so NSE, NYSE and crypto calendars line up; `null`/`N/A` when two symbols share fewer than 20 returns |
| `timeframes <SYMBOL>` (`mtf`) | 1M–5Y return, volatility, RSI, SMA/MACD signals |
| `returns <SYMBOL> [--json\|--csv]` | 1W–5Y returns. The annualized figure (CAGR) uses the calendar time between the first and last bar, and is shown only for spans of a year or more (`null` otherwise) |
| `patterns <SYMBOL> [-p]` | Candlestick patterns (doji, hammer, engulfing, morning star) |
| `support <SYMBOL> [-p]` | Pivot points (R1–R3, S1–S3) from the last completed daily session (named on screen), whatever `-p` is, + Fibonacci levels |
| `fibs <SYMBOL>` | Fibonacci retracement with a "you are here" marker, for the last 21, 63 and 126 trading days (1, 3 and 6 months of daily bars). Always reads a year of daily bars; `-p` is accepted but ignored |
| `volume <SYMBOL> [-p]` (`vol`) | OBV, A/D line, volume ratio, volume bars |
| `gaps <SYMBOL> [-p]` | Filled and unfilled price gaps |
| `stoploss <SYMBOL> [-e, --entry PRICE] [--capital AMT] [--risk-pct PCT] [-p]` | ATR (1.5x/2x/3x), percentage and Chandelier stops, position sizing (shares and position value for risk budget) and R:R targets |
| `forecast <SYMBOL> [-d DAYS] [-p]` | Linear trend projection with 95% bands — a projection, not a prediction |

`--explain` on `verdict`, `technical`, `score` and `options oi` adds plain-English notes for beginners.

### Actions

`verdict`, `deep` and a bare `stockwise SYMBOL` start with one line from the
rule-based action engine, followed by its reasons:

```text
ACTION: BUY · medium confidence · invalid below ₹2,410.50 · review by 21 Feb 2027
  • Composite score 71 (65 or more)
  • Price above its 200-day average (₹2,388.10)
  • P/E 24.3 within 1.2x the sector P/E (22.0)
  • Size: at most 6.2% of the portfolio, about 25 shares (risk_per_trade)
  Methodology 2 (long horizon). Rule-based educational signal from StockWise's published methodology, ...
```

A stock you don't hold gets BUY, WATCH or AVOID; one you hold gets ADD, HOLD,
TRIM or EXIT. Below 60% data coverage the line is
`ACTION: NO CALL — missing: ROE, D/E` and nothing is guessed. A daily close
below the invalidation price invalidates the action; look again by the review
date. For unheld stocks the ATR reference is the last close; for holdings it is
the highest close of the last 20 sessions, so the stop trails price up. On the
default long horizon the invalidation is the higher of the 200-day average and
reference − 2.5 × ATR(14). `--horizon short` (on `verdict`, `portfolio review`
and `picks`) sets the invalidation at the higher of reference − 1.5 × ATR(14)
and the 20-day low, reviews in 10 sessions, and also asks for a price above the
50-day average and rising volume before a BUY. Small caps (below ₹5,000 crore or $2 billion) face
extra gates: promoter pledge above 10%, thin trading, more than 2 price-band
hits in 20 sessions (NSE/BSE), an auditor red flag, or exchange surveillance
make them AVOID. The same action appears in the Action columns of
`portfolio`, `portfolio checkup` and `picks`, in [`portfolio
review`](#portfolio-review), as an `action` object in `--json`, and in the MCP
tools `stock_action` and `portfolio_review`. Every rule and threshold is in
[methodology.md](methodology.md#actions-methodology-1). Actions are rule-based
educational signals; StockWise is not a SEBI-registered investment adviser or
research analyst, and losses are possible ([DISCLAIMER.md](../DISCLAIMER.md)).

## Market

> **Guide:** [Check a stock or fund fast](guide-research.md#2-get-the-price).

| Command | Description |
|---------|-------------|
| `movers [--universe INDEX] [--cap TIER] [--json\|--csv]` | Top gainers and losers across the liquid stocks of BSE (India) or the S&P 1500 (US), or another universe (see below), with a cap-tier column. The table opens with an "As of" line: the local time, the session date of the latest trade and whether the market is open |
| `sectors [--json\|--csv]` | Sector ETF/index performance with bars |
| `heatmap [--universe INDEX] [--cap TIER]` | Heatmap of the liquid stocks of BSE (India) or the S&P 1500 (US), or another universe; a scan of more than 60 stocks shows the 30 biggest gains and the 30 biggest falls. Stocks without a day change are listed last with "—" |
| `sentiment [-m in\|us]` | Fear & Greed style gauge for the market (`-m`, default the global market): India VIX, NIFTY 50 day move and 1-month trend, NIFTY MIDCAP 50 against NIFTY 50, with BANK NIFTY shown, for India; VIX, S&P 500, Russell 2000 and Nasdaq for the US; plus the wording of market headlines (whole words, negation-aware). India VIX bands sit higher than the VIX's: below 14 extreme greed, 14–18 greed, 18–24 neutral, 24–32 fear, above 32 extreme fear (US: 15 / 20 / 25 / 35). A reading of mood, not a signal |
| `conditions [--universe INDEX] [--json]` | Where the market stands, as readings: **trend** (the main index against its 200- and 50-day averages, distance from its 52-week and 10-year highs, and the share of days in 10 years when it sat lower against the 200-day), **volatility** (India VIX or VIX against 5 years; India and US), **breadth** (share of the scanned stocks above their 200- and 50-day averages, near 52-week highs and lows, up on the day) and **valuation** (aggregate and median P/E, earnings yield against the 10-year government yield for India and the US, median dividend yield). Breadth and valuation cover the same stocks as `picks`. They have no price history to look back over, so each run (and each daily `daemon longterm` run) saves the session's figures in `cache.db` (one per session, market and `--universe`; `cache clear` leaves them) and, once 20 earlier sessions are on record, shows the share of them that were lower, plus the change since the session before. Readings of the data: not a forecast and not a signal to invest or to wait |
| `news <SYMBOL> [--days N] [--limit N] [--json]` | Headlines from Yahoo Finance and Google News (searched by company name in the stock's locale, last `--days`, default 3) plus exchange filings (NSE for `.NS`, BSE for `.BO`, SEC EDGAR for US tickers), de-duplicated, newest first, with links and local times; the header counts the sources that answered |
| `rotation` | Sector indices (India) or sector ETFs (US) with the day change and relative strength against NIFTY 50 / S&P 500 over 1, 3 and 6 months of daily closes, strongest over 3 months first. Leading: ahead of the benchmark over both 1 and 3 months; Lagging: behind over both; "—" where a quote or history is missing |
| `sectorcmp <SECTOR1> <SECTOR2>` | Two sectors head to head: price, day change, 1/3/6-month returns and relative strength against the benchmark, P/E, 50-day average and 52-week range ("—" when missing) |
| `peers <SYMBOL>` | Same-sector peers on P/E, P/B, market cap |
| `insider <SYMBOL>` | Insider trades from the regulator's filings — NSE PIT disclosures for Indian stocks, SEC EDGAR Form 4 for US stocks (set `STOCKWISE_SEC_USER_AGENT` if SEC answers 403) — falling back to Yahoo Finance `insiderTransactions` (labelled) when those fail or list nothing; open-market acquisitions and disposals are totalled. Then analyst consensus and insider-related news |
| `ipo [SYMBOL] [--board all\|mainboard\|sme] [--capital 2.5L] [--recent N] [--rhp FILE] [--rhp-template] [--json]` | Indian IPOs from NSE, with BSE's public-issue pages filling whatever NSE does not supply (field by field; `meta.field_sources` and the footer say which fields came from BSE), with a rule-based verdict per issue: **APPLY**, **APPLY – SELL ON LISTING**, **WAIT** (with the trigger, e.g. "APPLY if QIB ≥ 12.4x by 3 PM IST on 25 Nov 2026"), **AVOID** or **NO CALL** (coverage below 60%); LISTED hands a trading issue to the stock action engine. Without a symbol: issues open now and upcoming (terms, QIB/NII/retail subscription, verdict cell and first reason), recent listings with their listing-day and current return, and the IPO-market regime (recency-weighted median listing-day return of the last 180 days, 8 listings minimum). With a symbol or company name, the verdict is the first line, then the listing-day model (expected gain, P(close below issue), 10th–90th percentile, fit stats), expected value per minimum application by category, the listing-day plan, anchor lock-in dates, issue terms, valuation at the upper band against listed peers (from `ipo/<SYMBOL>.toml`, the RHP figures you save; `--rhp-template` prints the file, `--rhp` reads another), subscription and demand by price, allotment odds from application counts (with `--capital`, what that amount buys), unofficial GMP and documents. `--json` leads with the `verdict` object. Every verdict is logged for `ipo review`. Rules: [methodology.md](methodology.md#ipo-verdicts-methodology-ipo-1) |
| `ipo review [--since 1y] [--json\|--csv]` | The IPO verdicts' track record: each issue's last verdict before close, its listing-day close and ~3-month return against the issue price, and NIFTY 50 over the same three months; per verdict: issues, hit rate, mean and median listing-day return, the model's mean expected gain, mean 3-month return and excess over NIFTY |
| `ipo calibrate [--csv FILE] [--since DATE] [--save] [--print-const] [--json]` | Refit the listing-day model by robust regression on NSE mainboard issues listed since `--since` (default three years): final QIB/NII/retail subscription from NSE, listing-day close from Yahoo (or a CSV with `symbol,listing_date,issue_price,qib_x,nii_x,retail_x[,listing_open,listing_close]`). Prints the coefficients, n, R², residual sd and an in-sample P(loss) check; `--save` keeps the fit as `ipo_model.json` in the data directory, which verdicts then use; the dataset is written to `ipo_calibration.csv` |
| `earnings` | Next results date of each stock in the portfolio and watchlist (NSE board meetings for Indian stocks, else Yahoo's `calendarEvents`; `~` marks an estimate), soonest first, with trailing and forward EPS |
| `surprise [--universe INDEX]` | Latest reported quarter of each stock: actual EPS against the analyst estimate and the surprise %, largest first (Yahoo `earningsHistory`; one request per stock, so a scan of more than 50 stocks covers the 50 largest by market cap) |
| `matrix [--universe INDEX]` | Quality vs growth quadrants |
| `picks [--universe INDEX] [--cap TIER] [--horizon long\|short] [--top N] [--min-mcap N] [--min-adv N] [--json\|--csv]` | Scores the stocks of the scan (the liquid stocks of BSE for India, the S&P 1500 for the US) that clear a size and liquidity floor (India ₹500 crore and ₹2 crore traded a day; US $1 billion and $5 million; lower floors for `--cap small` and `--cap micro`), from 0 to 100, and lists the highest. Five pillars, each a percentile rank among the stocks scanned: **value** (earnings yield, book value to price, dividend yield), **quality** (return on equity, profit margin, low debt), **growth** (forward against trailing EPS with 3+ analysts, revenue growth), **trend** and **risk** (low volatility, drawdown and beta). The leaders of a first pass get their fundamentals and a year of prices before the final ranking. `--horizon long` (default) weights value and quality; `--horizon short` gives trend 45% (3-month momentum per unit of volatility, the 50-day average, the 52-week range, rising volume) and risk 20%. Losses and negative book value rank last; figures too good to be true (P/E below 3, ROE above 80%, yield above 15%) are listed apart to check; stocks with less than 60% of their data stay out; with a positive `--top`, at most 3 per known sector (`--top 0` has no limit). Every list is logged for `picks review`. The finalists get an Action column with their [action](#actions) (also with `--cap small --horizon short`). Method: [methodology.md](methodology.md) |
| `picks review [--since 90d] [--horizon long\|short] [--json\|--csv]` | The track record: every logged pick from the day it was first listed, with its return, the benchmark's return over the same days (NIFTY 50 for India, S&P 500 total return for the US) and the difference, plus the share that rose, the share that beat the benchmark, and mean and median returns. Price returns adjusted for splits since listing (a pick whose splits can't be checked is left out); dividends not included. The benchmark is measured from the level logged with each list to its level now (both live), so both legs compare the same moments; without those levels no benchmark figure is shown. `--since` takes up to 10 years |
| `screen <CATEGORY> [--universe INDEX] [--cap TIER] [--json\|--csv]` | `undervalued` (P/E<15), `growth` (forward EPS > trailing), `dividend` (yield>2%), `momentum` (top gainers), `bluechip` (>$100B / >₹5L Cr). The table shows the first 50 matches; `--json` and `--csv` carry them all |
| `dividends <SYMBOL> [--json]` | Yield, income projection, 10-year DRIP growth |

**Default India universe:** without `--universe`, an India scan covers every
stock listed on BSE that traded ₹10 lakh or more on the last session (about
1,900 stocks), read from BSE's end-of-day file (see below), plus every NIFTY
500 member. NIFTY 500 members keep their NSE symbol (`RELIANCE.NS`); the rest
are BSE symbols (`XYZ.BO`).
When that file is unavailable the scan covers NIFTY 500.

**Other markets** scan built-in index lists (snapshots, refreshed each
release): the S&P 1500 for the US (S&P 500, MidCap 400 and SmallCap 600), FTSE
350 for London, DAX and MDAX for Germany, Nikkei 225 for Japan, the Hang Seng
Index for Hong Kong, the Straits Times Index for Singapore, the S&P/TSX
Composite for Canada and the S&P/ASX 200 for Australia.

**`--universe`** (Indian market; `movers`, `heatmap`, `screen`, `picks`,
`matrix`, `surprise`, `longterm`): `all` is every stock listed on BSE (about
4,500, SME and thinly traded ones included) and `bse` is BSE's main board
(groups A and B, about 2,500), both from the same file. Or scan the current
members of an NSE index — `nifty50`, `niftynext50` (`next50`),
`nifty100`, `banknifty` (`niftybank`), `niftyit`, `nifty500`, `midcap150` or
`smallcap250`. NSE's website lists the members and prices them in one bulk
request (all 500 of `nifty500`), whichever data provider you use. `movers` and
`heatmap` rank from that snapshot alone, so they work when Yahoo Finance is
unreachable; `screen` needs P/E and market cap too, so it asks Yahoo for them
and falls back to the snapshot (no P/E or market cap) when Yahoo fails. Every
scan also saves the prices into `cache.db` (`market_snapshots`) and the index
members for the cap tiers below. When NSE can't be reached or
refuses, StockWise says so on stderr and reads the members from the CSV that
niftyindices.com (NSE Indices Ltd.) publishes for each index, then prices them
on Yahoo Finance; that list is alphabetical, so only the MIDCAP 150 and
SMALLCAP 250 bands are saved from it. When niftyindices.com fails too, it scans
a built-in list (the saved or built-in NIFTY 500 for `nifty500`, the 30 large
caps for `nifty50` / `nifty100`, a built-in member list for `banknifty` and
`niftyit`); `niftynext50`, `midcap150` and `smallcap250` have no built-in list
and fail with the reason.

**End-of-day prices (India):** BSE's daily bhavcopy is a public CSV of the
whole exchange's day. It lists the stocks of the default, `bse` and `all`
universes. Once it holds the last completed session, `movers` and `heatmap`
(without a `--cap` tier) take every stock's close from it outside market hours
instead of asking Yahoo Finance, and they fall back to it when Yahoo does not
answer. The file is downloaded once
per trading day (BSE posts it in the evening, IST) and kept in `cache.db`;
`stockwise cache clear bhavcopy` drops it. It has prices and volume only: no
P/E, market cap or intraday prices.

**Cap tiers** (`mega`, `large`, `mid`, `small`, `micro`): a badge such as
`[Large]` next to the market cap in `quote`, `deep`, `analyze` and `compare`
(and a `Cap` column in `movers`, `screen` and `portfolio`), and a `cap_tier`
field in their `--json` / `--csv` (`unknown` when there is no market cap, or
no exchange rate for the currency). Indian stocks follow the SEBI/AMFI ranks
when the index members are known: NIFTY 100 is large (the top 10, or above
₹5,00,000 Cr, is mega), NIFTY MIDCAP 150 is mid, NIFTY SMALLCAP 250 is small
and any other NSE stock outside NIFTY 500 is micro (BSE symbols, `XYZ.BO`, are always tiered by the rupee cut-offs below). Scanning those indexes with
`--universe` saves
their members in `cache.db` (`stockwise cache clear members` forgets them);
until then, and offline, rupee cut-offs decide (large from
₹1,00,000 Cr, mid from ₹33,000 Cr, small from ₹10,000 Cr). Every other market
converts the market cap to dollars at the built-in approximate rates (London
pence `GBp` and Japanese yen `JPY` included): mega from $200B, large from
$10B, mid from $2B, small from $300M. A display and filtering aid, not a
regulatory classification.

**`--cap all|mega|large|mid|small|micro`** keeps one tier in `movers`,
`heatmap` and `screen`. With a tier, `-m in` and `-m us` scans read fresh
(under 30 minutes old) `market_snapshots` rows from `cache.db` when `cache
warm` or the worker has stored most of the market's list, with no request at
all; otherwise they scan live. There is no per-sector drilldown command yet,
so `--cap` has nothing to filter there.

## Funds & F&O

> **Guide:** [Check a stock or fund fast](guide-research.md#6-check-a-mutual-fund) covers funds and SIP returns.

| Command | Description |
|---------|-------------|
| `mf search <QUERY...> [-n N] [--json]` | Find Indian mutual-fund schemes by words in the name, fund house, category or ISIN; add `direct growth` to narrow. Shows the AMFI scheme code the other `mf` commands take |
| `mf nav <CODES...> [--json]` | Latest NAV and date, fund house, category, ISINs, and the plan's expense ratio (TER) with the other plan's for comparison |
| `mf history <CODE> [-p PERIOD] [--json]` | NAV chart for the period (default `1y`), expense ratio (TER), trailing 1M–10Y returns (annualized from 1Y), since-inception CAGR, max drawdown. `--json` includes every NAV in the period |
| `mf sip <CODE> <AMOUNT> [-p PERIOD] [--json]` | Monthly SIP from NAV history (default `5y`): value-vs-invested chart, gain, units, and XIRR on the actual purchase dates |
| `mf screen [CATEGORY...] [--plan direct\|regular\|all] [-s 1y\|3y\|5y\|ter] [--max-ter PERCENT] [-n N] [--max N] [--json]` | Bare: list categories with their active scheme counts. With words (`flexi cap`, `elss`, `liquid`, `equity etf`): rank the category's growth-option schemes by return (default 3Y CAGR, direct plans) or by expense ratio (`-s ter`, lowest first — with more matches than `--max`, the cheapest are screened), with a TER column. `--max-ter 1.0` keeps only funds whose disclosed TER is at most 1% |
| `mf bleed [--return PERCENT] [--json]` | Held Regular-plan funds vs their Direct growth twin: TER gap (AMFI disclosure), yearly rupee cost, 10-year compounded cost at a stated gross return, units that redeem tax-free inside the unused Sec. 112A exemption, and breakeven months for younger units |
| `mf overlap <A> <B> [--json]` | Shared top holdings, holdings and sector overlap, and expense ratios of two funds or ETFs, e.g. `mf overlap SPY QQQ -m us` |
| `options chain <SYMBOL> [-e, --expiry DATE] [-n, --strikes N] [--rate %] [--div-yield %] [--json]` | Calls and puts around the money: last price, open interest (and change on NSE/BSE), volume, IV, delta, gamma, theta, vega, rho, and the lot size. Default: nearest expiry, 10 strikes each side (`-n 0` for all) |
| `options pcr <SYMBOL> [-e DATE] [--json]` | Put/call ratio by open interest and by volume, and the three strikes with the most call and put open interest |
| `options oi <SYMBOL> [-e DATE] [-n N] [--json] [--explain]` (`open-interest`, `fno oi`) | Open-interest analytics: PCR, Max Pain, top call resistance and put support walls with institutional build-up classification (`LB`, `SC`, `SB`, `LU`), and aggregate volume breakdown. Sentiment is read for the underlying: put buying (`LB` on a put) is bearish and put writing (`SB`) is bullish; `--json` carries `calls_sentiment` and `puts_sentiment` |
| `options maxpain <SYMBOL> [-e DATE] [--json]` (`max-pain`) | The settlement strike at which open options pay their holders least, with the payout curve around it |
| `options payoff <SYMBOL> [--leg LEG]... [-s PRESET] [-w WIDTH] [-e DATE] [-l LOTS] [--lot-size N] [--rate %] [--div-yield %] [--iv %] [--today] [--json]` | A multi-leg position priced from the live chain: legs with price source, IV and delta; net debit/credit; max profit and loss (or "unlimited"); breakevens; probability of profit; position greeks; and a braille P/L chart at expiry (green/red), plus today's curve with `--today`. Analysis only — no orders |
| `options <SYMBOL> [-p]` / `options moves <SYMBOL> [-p]` | Expected 1/5/10/30-day moves and OTM strike ideas from realized volatility |

MF periods (`-p`) are `<n>mo`, `<n>y` or `max`, e.g. `6mo`, `3y`, `10y`. Expiries
(`-e`) are `YYYY-MM-DD`.

**`options payoff` legs and presets**

- A leg is `buy|sell [QTY] call|put STRIKE [@PRICE]` or `buy|sell [QTY] stock [@PRICE]`:
  `--leg "buy call 24500" --leg "sell call 24800"`, `--leg "sell 2 put atm-1"`,
  `--leg "buy stock @2450"`. `STRIKE` is a listed strike, or `atm`, `atm+N`,
  `atm-N` (N listed strikes from the money). An unlisted strike is an error that
  names the nearest listed ones.
- Prices: the bid/ask mid where both are quoted, else the last trade (flagged
  `ltp` — it can be stale), else the leg needs `@PRICE`. A stock leg is priced at
  the current underlying value (for an index that stands in for the future).
- Presets around the at-the-money strike, `W` = `--width` (default two strike
  steps; a strike that isn't listed snaps to the nearest one, with a note):
  `bull-call` (buy ATM call, sell ATM+W), `bear-put` (buy ATM put, sell ATM−W),
  `straddle` (buy ATM call and put), `strangle` (buy ATM−W put and ATM+W call),
  `iron-condor` (sell ATM±W, buy ATM±2W wings — a credit), `butterfly` (buy
  ATM±W calls, sell two ATM calls), `covered-call` (stock at spot, sell ATM+W
  call). `--strategy` and `--leg` combine.
- Sizes: every amount is per position — per-unit P/L × lot size × `--lots`. Lot
  sizes come from the chain (NSE/BSE order-book quantities are whole lots; US
  contracts are 100 shares). After hours BSE shows no order book: pass
  `--lot-size`, otherwise amounts are per unit and the output says so.
- Probability of profit: the chance the expiry price lands where P/L > 0 if the
  price is lognormal with the at-the-money IV (or `--iv`) and risk-neutral drift.
  It is a model, not a forecast. Costs (brokerage, STT, exchange charges) are not
  included. StockWise never places orders from this command.

**Where the data comes from, and what has none**

- **NAVs**: AMFI's public [NAVAll.txt](https://portal.amfiindia.com/spages/NAVAll.txt),
  every Indian scheme, cached in the data directory (`cache/`) and re-downloaded
  after 12 hours. NAV history: [api.mfapi.in](https://www.mfapi.in), a free
  community mirror of AMFI data, cached the same way. If a download fails, a
  stale cached copy is used with a warning on stderr.
- **Returns** are point-to-point NAV returns of the growth option, before tax
  and exit load. `mf sip` buys at the first NAV on or after each monthly due date.
- **Expense ratios (TER)**: AMFI's monthly TER disclosure
  ([amfiindia.com/ter-of-mf-schemes](https://www.amfiindia.com/ter-of-mf-schemes)),
  fetched from the API behind that page as one spreadsheet export for the month
  (`/api/populate-te-rdata-revised?MF_ID=All&Month=MM-YYYY&strCat=-1&strType=-1&excel=true`,
  ~3 MB; the previous month if the current one isn't published yet). It lists
  each scheme's TER per day for the regular and direct plan — from April 2026
  split into base expense ratio, brokerage, transaction costs and statutory
  levies. StockWise keeps each scheme's latest day, caches the result in
  `cache/amfi_ter.json` for a day, and uses a stale copy with a warning if AMFI
  is unreachable. The disclosure has no AMFI scheme code, so schemes are joined
  on their name (case and punctuation ignored); a plan the scheme doesn't offer
  (AMFI writes 0) has no TER. When TERs can't be loaded, `mf nav`/`history`/`screen`
  still print NAVs and returns with a note; `--sort ter` and `--max-ter` fail
  instead. In JSON, `ter` holds `plan`, `total`, the breakdown, both plans'
  totals and the disclosure `date`, as fractions (`null` when not disclosed).
  `mf overlap` shows Yahoo's expense ratio for the funds it covers.
- **Holdings** (`mf overlap`) come from Yahoo Finance, which publishes the top 10
  holdings and sector weights for US ETFs and mutual funds (SPY, QQQ, VTI,
  VFIAX, ... with `-m us`) and for some funds under Yahoo's `0P…` IDs. NSE-listed
  ETFs such as NIFTYBEES and AMFI scheme codes have no holdings data — StockWise
  says so instead of guessing. With only the top 10 known, the holdings overlap
  is a lower bound; sector weights cover the whole fund.
- **Option chains**: in the Indian market (the default), NSE's public
  option-chain API — index options on NIFTY, BANKNIFTY, FINNIFTY, MIDCPNIFTY and
  NIFTYNXT50 (`^NSEI` and `^NSEBANK` work too) and NSE F&O stocks such as
  RELIANCE. NSE serves browser sessions only: StockWise loads its option-chain
  page once for cookies, makes two API calls, and stops with a clear error if NSE
  refuses — wait a minute and retry. `SENSEX`, `BANKEX` and `SENSEX50` (also
  `^BSESN`, `BSE-BANK.BO`) and BSE stock options as `SYMBOL.BO` come from BSE's
  derivatives API, the one behind bseindia.com's option-chain page: one session,
  calls 400 ms apart, answers cached for a minute in `cache/`, no retries. BSE's
  site sometimes refuses scripted requests (HTTP 403) — StockWise then says so;
  wait and retry, or use the website. BSE stock options are rarely traded. With
  `-m us`, Yahoo Finance (delayed quotes) for US stocks and ETFs.
- **Lot sizes**: NSE and BSE quote order-book quantities in whole lots, so the
  lot is their greatest common divisor across the chain (with at least five
  quotes; a divisor of 1 counts as unknown); US contracts are 100 shares. `options chain` shows it and `--json`
  has `lot_size` (`null` when unknown).
- **IV and greeks**: Black-Scholes-Merton. IV is solved from the bid/ask mid
  (else the last trade) at the data timestamp; when that fails, the source's IV
  is shown (`iv_from` in JSON). Risk-free rate defaults to 6.5% (India) / 4.0%
  (US) — pass `--rate` with the current T-bill yield. Theta is per calendar day,
  vega and rho per percentage point. NSE options are European; US equity options
  are American, so their greeks are the usual European approximation.
- **PCR and max pain** describe positioning for one expiry; they are not
  forecasts.

## Commodities & bonds

| Command | Description |
|---------|-------------|
| `commodity [--json\|--csv]` / `commodity spot` | Gold and silver in USD per troy ounce and in ₹ per gram, per 10 g (gold) and per kg (silver), with the day's change. ₹ prices are international prices converted at USD/INR — before Indian import duty and GST |
| `commodity futures [--json\|--csv]` | COMEX gold and silver futures (GC=F, MGC=F, SI=F, SIL=F) with the front contract month and volume |
| `commodity etfs [--json\|--csv]` | Gold and silver ETFs on NSE: GOLDBEES, SETFGOLD, HDFCGOLD, GOLDIETF, GOLD1, GOLDCASE, SILVERBEES, SILVERIETF, HDFCSILVER, SILVER1 |
| `commodity sgb [--json\|--csv]` | Sovereign Gold Bonds trading on NSE: price per unit (1 g), premium or discount to 1 g of gold at the international spot price, current yield of the 2.5% interest, maturity and volume |
| `bonds [--json\|--csv]` / `bonds yields` | Benchmark yields by tenor — 91/182/364-day T-bills, G-secs from 1–2 to 28–30 years (India 10Y = the 9Y–10Y bucket) and state loans (SDLs) |
| `bonds list [QUERY] [--json\|--csv]` | Government of India securities outstanding: ISIN, name, maturity, NSE symbol, outstanding stock. `QUERY` is a maturity year (`2033`), part of a name (`GS 2036`), an ISIN or an NSE symbol |
| `bonds calc [SECURITY] (--price P \| --ytm Y) [--coupon C --maturity DATE] [--settle DATE] [--dirty] [--tbill] [--json]` | Price ↔ yield to maturity, accrued interest, dirty price, current yield, Macaulay and modified duration, next coupon. `SECURITY` (`718GS2033`, `"7.18% GS 2033"`, `IN0020230085`) is looked up in RBI's list for its coupon and maturity; or give `--coupon` and `--maturity`. `--dirty`: the price includes accrued interest (NSE quotes G-secs that way). `--tbill`: a Treasury bill, priced on its discount yield. Settlement defaults to today in India |

**Where the data comes from, and what has none**

- **Gold and silver spot**: Yahoo Finance has no free spot quote, so spot is
  the COMEX front-month future (`GC=F`, `SI=F`, USD per troy ounce) converted at
  `USDINR=X` (1 troy ounce = 31.1035 g). It excludes Indian import duty and GST,
  so jewellers' rates, IBJA rates and MCX prices are higher.
- **MCX futures** have no free public feed (MCX's site is bot-protected and its
  data is licensed), so StockWise shows COMEX contracts and labels them.
- **ETFs** are NSE listings via Yahoo, in ₹ per unit; each ETF holds a
  different amount of metal per unit, so compare % moves, not prices.
- **SGB prices** come from NSE's Sovereign Gold Bond list
  (`/api/sovereign-gold-bonds`) through the same polite NSE session as the
  [NSE provider](data-providers.md#nse-and-bse), cached for 5 minutes and kept
  on disk. When NSE refuses (it often blocks scripts), the last saved list is
  shown with its date; with none, every outstanding tranche is listed **without
  a price** — never a guessed one. Tranche details (series, issue date, issue
  price, maturity = issue + 8 years, early redemption from year 5) are bundled
  from public tranche tables; 33 of the 44 issue dates were checked against
  RBI press releases, and RBI moves a maturity a day or two earlier when it
  falls on a holiday.
  Their NSE symbols come from a public list and are only shown when NSE's own
  list is unavailable. No new SGBs have been issued since February 2024.
  SGBs you hold are valued from the same list in `portfolio` and the other
  holdings commands (see [Mutual funds and SGBs](#mutual-funds-and-sgbs)).
- **Yields**: CCIL's [tenor-wise indicative yields](https://www.ccilindia.com/tenorwise-indicative-yields),
  end of day, cached for 6 hours; when CCIL can't be reached, the five
  benchmark G-sec yields and T-bill cut-offs on the [RBI home page](https://www.rbi.org.in/).
  CCIL and RBI data are for personal, non-commercial use.
- **Outstanding G-secs**: RBI's [list of Government of India securities](https://www.rbi.org.in/scripts/bs_viewcontent.aspx?Id=1956),
  cached for a day.
- **Prices of individual G-secs and T-bills** are not shown: neither CCIL nor
  RBI publishes them as a plain page, and NSE's live debt quotes have no
  documented API. Enter the price your broker shows into `bonds calc`.
- **Bond math**: half-yearly coupons on the maturity date's day, 30/360
  (European) day count, yield compounded half-yearly, simple interest in the
  final coupon period — the G-sec convention. T-bill yield =
  `(100 − price) / price × 365 / days`. Floating-rate bonds (FRBs) are not
  supported.

## Strategies & bots

> **Guide:** [Backtest and practise on past markets](guide-practice.md) and [Set up alerts and the daemon](guide-alerts.md).

| Command | Description |
|---------|-------------|
| `backtest <SYMBOL> [-s STRATEGY] [-P NAME=VALUE] [-p] [--json\|--csv]` | Backtest one stock after costs: net and gross return, buy & hold, CAGR, drawdown, Sharpe/Sortino, exposure, trades. Fills at the next bar's open. Strategies: `rsi` (default), `macd`, `sma`, `bb`, `vwap`, `mr`, or your own TOML file. Default `2y` |
| `backtest portfolio <SYMBOLS...> [-s] [--weights equal\|inverse-vol\|0.5,0.3,...] [--rebalance none\|monthly\|quarterly] [--json\|--csv]` | One strategy across several stocks sharing the capital; combined equity curve and per-symbol contribution |
| `backtest walkforward <SYMBOL> [-s] [--train 1y] [--test 3mo] [--json\|--csv]` (`wf`) | Tunes the strategy's `[grid]` on rolling train windows and reports only the unseen test windows, with an overfitting verdict. Default `5y` |
| `backtest strategies [FILES...] [--json]` | List built-in and your own strategies, or check strategy files (exit 1 on errors) |
| `sip <SYMBOL> <AMOUNT> [-p] [--json]` | Monthly SIP/DCA simulation with value-vs-invested chart and XIRR. Default `5y` |
| `whatif <SYMBOL> <AMOUNT> <YYYY-MM-DD> [--json]` | What a lump sum on a past date is worth today |
| `intraday [-s STRATEGY]` (`bot`) | 9-strategy intraday signal scanner: sector heat, trend breadth, a table of regime, score and strategy agreement, and the strategies behind the strongest signals. No position sizes, entries, stops or targets. `-s` adds a strategy file's rules as a tenth signal. Old `CAPITAL TARGET%` arguments are accepted and ignored |
| `longterm [MONTHLY_BUDGET]` | 6-pillar scores (0–100) for the market's stock universe, with the facts that added or took away points ("P/E 9.8 (below 12)", "Analyst mean rating 1.4/5 (1 = most positive)"), a data-coverage figure, a risk level (Lower / Moderate / Higher) and rule-based moat signals. A pillar with no inputs shows a dash and the composite shows `n/a` when under half of the weight had data. Also an illustration at an *assumed* annual return set from the score (5- and 10-year growth, the monthly SIP that would reach 10 lakh / 1 million, your `MONTHLY_BUDGET` compounded) and a simulated 5-year range (10th, median, 90th percentile). It describes data; it is not a recommendation and gives no entry, stop or target. No `--json` yet. Wording changes in 0.8.1: the analyst lines no longer say "buy consensus" / "sell consensus" / "upside", the risk tier names `Conservative` / `Aggressive` became `Lower` / `Higher`, and "Best case" / "Worst case" became the 90th / 10th percentile |
| `daemon intraday [CAPITAL] [--once]` | Paper-trading loop on the NSE clock (IST, holidays included; `-m us` uses NYSE hours in ET): scan, enter at the open (0.5% sizing budget per trade, charges included; half-Kelly after 30 net paper trades), mark positions every 60s with a 1.5 × ATR trailing exit level, book half at T1, square off 15 minutes before the close, settle into `sim` history with P&L after charges and slippage, then wait for the next trading day (`--once` exits after one session). Survives network blips, resumes the day after a restart, and squares off on Ctrl+C. **Never sends broker orders** |
| `daemon longterm` | Post-market scan: the highest long-term scores, portfolio value and wealth snapshot, triggered alerts, holdings priced below their average cost, and optional AI research notes |
| `sim start [CAPITAL] [TARGET%]` | Start a paper session with the bot's picks (default ₹25,000, 2%) on a trading day before the close. Prints the cost model and sizing, and saves the cost model with the session. Costs: `STOCKWISE_SIM_COSTS`, `STOCKWISE_SIM_BROKERAGE`, `STOCKWISE_SIM_SLIPPAGE_BPS` (see [bots.md](bots.md#charges-and-slippage)) |
| `sim status` / `history [--json]` | Live P&L after estimated charges / record, charges paid and a paper-record rating (after 20 dates and 30 trades) |
| `sim settle [--now]` | Settle open sessions from that day's 5-minute bars: stop or target first, else exit at the 15:15 bar. Charges and slippage come out of each trade with the session's saved cost model. Today's session waits for the close unless `--now` |
| `sim reset [--yes]` | Delete simulator history after confirmation (a timestamped backup is kept) |
| `sim replay list [--json]` (`scenarios`) | Built-in historical replays: `covid-crash-2020`, `gfc-2008`, `adani-2023`, `dotcom-2000`, `bull-run-2021`, `rate-hikes-2022`. See [simulator.md](simulator.md) |
| `sim replay start SCENARIO\|file:PATH.toml [--capital] [--costs] [--json]` | Start a replay: daily history is fetched once, then you trade day by day and only ever see bars up to the replay date |
| `sim replay start --from DATE --to DATE --symbols A,B,... [--benchmark SYM]` | Your own replay window in the active market (`-m in`/`-m us`); benchmark NIFTY 50 / S&P 500 by default |
| `sim replay buy SYMBOL QTY [--stop P] [--target P]` / `sim replay sell SYMBOL [QTY]` | Queue an order for the next day's open (backtest cost models; long only). `sell` without a quantity sells the whole position |
| `sim replay next [N]` / `sim replay status` / `sim replay chart SYMBOL\|index [--days]` | Move N trading days (stops and targets on each day's range); account, positions, orders and the market on the replay date; a chart up to it |
| `sim replay cancel [ID]` / `sim replay set SYMBOL [--stop P] [--target P]` | Cancel pending orders; move a position's stop or target |
| `sim replay end [--json]` | Finish: 0–100 score and grade, badges, streaks, and what actually happened |
| `sim challenge list` / `sim challenge start NAME` / `sim challenge status` / `sim challenge end` `[--json]` | Goal-based challenges with hard and soft rules over the running replay or your live paper sessions: `beat-the-index`, `steady-hands`, `position-limit`, `stop-loss`, `sip-discipline`, `crash-survivor`, or `file:PATH.toml` |
| `sim profile [--json]` | Badges (earned and how to earn the rest), streaks, best score per scenario, recent runs |
| `sim card [--svg FILE] [--text]` | Your last run as a text card, or a 640 × 360 SVG image — with the paper-trading disclaimer |
| `report <intraday\|longterm\|portfolio\|SYMBOL>` | Educational AI notes (signals, risks, what to watch) via local Ollama or an opt-in cloud model — see [ai.md](ai.md) |

Every `backtest` form also takes:

| Option | Default | |
|--------|---------|---|
| `-s, --strategy` | `rsi` | A built-in, a file in your strategies folder, `file:PATH.toml`, or `wasm:PATH.wasm` (a [WASM plugin](backtesting.md#wasm-plugins); `--features wasm-plugins` builds) |
| `-P, --param NAME=VALUE` | | Override a strategy parameter (repeatable) |
| `--costs` | `auto` | `auto`, `in-delivery`, `in-intraday`, `us`, `crypto`, `fx`, `none`. `auto` = crypto/FX models for coins and pairs, India delivery for INR stocks, US otherwise |
| `--brokerage` | preset | Per order: `20` (flat), `0.03%`, or `20,0.03%` (whichever is lower) |
| `--slippage-bps` | `5` | Slippage per fill, in basis points |
| `--fill` | `next-open` | `next-open` or `close` (fills at the signal bar's close — look-ahead) |
| `--capital` | `100000` | Starting capital in the stock's currency |
| `--json` / `--csv` | | Print the whole result as JSON, or the trades as CSV |

Costs, fills, the metrics, walk-forward and the strategy file format are in
[backtesting.md](backtesting.md).

## Portfolio

> **Guide:** [Import your trades and see true returns](guide-portfolio.md) and [Rebalance to target weights](guide-rebalance.md).

Data lives in the directory shown by `stockwise doctor`.

| Command | Description |
|---------|-------------|
| `portfolio` / `portfolio show [--base inr\|usd] [--json\|--csv]` (`p`) | Holdings with live P&L, XIRR and allocation; mutual funds at AMFI's NAV and SGBs from NSE ([below](#mutual-funds-and-sgbs)); holdings without a price show N/A. Mixed INR/USD holdings are totalled in `--base` (default: the market's currency). An Action column shows each stock's [action](#actions) (`HOLD (med)`, `NO CALL`) |
| `portfolio add <SYMBOL> <SHARES> <COST> [--date YYYY-MM-DD] [--currency CODE] [--charges AMT\|auto] [--force] [-m MARKET]` | Add a purchase as a new lot (default date: today). `--charges` records brokerage, taxes and fees paid, in total; `auto` estimates an NSE/BSE delivery buy's STT, stamp duty, exchange, SEBI and GST charges (not brokerage). `-m` picks the market for a bare ticker (`portfolio add VOD 300 70 -m uk` → `VOD.L`), and the confirmation names the market and its currency. `--currency INR` takes a price paid in another currency and converts it at that day's exchange rate (the pair's close on or before the purchase date) into the currency the instrument is quoted in; the rate used is printed, and the purchase is refused when Yahoo Finance gives no quote currency. A ticker Yahoo does not know is refused with "did you mean" names; `--force` saves it anyway (also on `watch add` and `alert add`). A mutual fund is `MF:<AMFI code>` or `MF:<ISIN>` (units and NAV per unit), kept as typed in every market |
| `portfolio sell <SYMBOL> <SHARES> [PRICE] [--date YYYY-MM-DD] [--lots N,N] [--charges AMT\|auto] [--preview]` | Record a sale, oldest lots first (FIFO). `--preview` shows the lots, tax and LTCG countdown and records nothing. `--charges` makes realized P&L net of the sale's costs (`auto` estimates them for an NSE/BSE delivery sell; not with `--preview`). Price defaults to the live price (a fund's latest NAV); a past `--date` needs a price. Refused while a split or bonus before the sale isn't applied to your lots. `--lots 3,1` sells from those lots (numbers from `portfolio lots`) in that order — specific identification for US and UK shares; Indian shares and funds always sell oldest first |
| `portfolio lots <SYMBOL> [--lot N [--date YYYY-MM-DD] [--qty Q] [--price P] \| --delete] [--undo-sale N] [--json]` | Lots, sales, dividends and XIRR of one holding, each numbered; `--json` prints the stored `{symbol, lots, sales, dividends}`. `--lot N` corrects a lot's date, quantity or price, or `--delete`s it without recording a sale; `--undo-sale N` drops sale N from the realised gains and puts its shares back as the lot they came from |
| `portfolio restore [N]` | Lists the last five saved versions of the portfolio (`portfolio.json.1` is the newest), or brings version N back. The version it replaces becomes backup 1, so a restore can be undone |
| `portfolio actions [SYMBOL] [--apply] [--undo] [--journal]` | Splits, reverse splits, bonuses, demergers and spin-offs (built-in table: Indian demergers, US spin-offs from Form 8937), cash for fractional merger shares, rights issues (reported only) and dividends since your purchases, including positions you have sold (from Yahoo; Indian bonus issues checked against NSE). Shows them; `--apply` adjusts lots, adds zero-cost lots for Indian bonus shares, adds lots of a demerged company at the official share of cost, records the cash for a merger fraction as a sale, records dividends, and is safe to repeat. Every `--apply` is journalled: `--journal` lists the entries, `--undo` takes back the newest |
| `portfolio demerger <PARENT> <CHILD> [--ratio A:B] [--cost-split PCT] [--date YYYY-MM-DD]` (`spinoff`) | Record a demerger or spin-off: each PARENT lot bought before `--date` keeps its quantity, date and `100 − PCT`% of its cost; a CHILD lot with the same purchase date gets A shares per B held and the rest. Ratio and split default to the built-in table. Shares sold after the record date get the same split, and child shares a broker snapshot already added are taken as the demerger's. Journalled; repeating it changes nothing |
| `portfolio rename <OLD> <NEW>` | A new ticker for the same company: lots, sales, dividends and imported trades move as they are (merged into NEW if held). Journalled |
| `portfolio return-of-capital <SYMBOL> <PER_SHARE> --date YYYY-MM-DD` (`roc`) | Cuts the cost of the lots bought before the ex-date by the amount per share, never below zero (the excess is printed as a gain to report). Journalled; repeating it changes nothing |
| `portfolio sync [BROKER] [--apply] [--json]` | Read-only: the broker's holdings against your lots, per symbol, with the likely cause (unapplied split or demerger, missing buy, missing sale) and the command that fixes it. `--apply` adds only the missing buys (dated today, estimated). Without BROKER: the reports the worker saves daily after the close |
| `portfolio list` | Your portfolios: the default one and every named one |
| `portfolio benchmark [NIFTY\|SENSEX\|SP500\|...] [-p 1y] [--base inr\|usd] [--json\|--csv]` | Time-weighted return against an index over the period, from the lots held each day: alpha, beta, tracking error, and attribution by holding and sector (see [below](#benchmark-and-attribution)) |
| `portfolio pme [NIFTY\|SPY\|VOO\|VWCE\|NIFTYBEES\|...] [--base inr\|usd] [--json\|--csv]` | Public Market Equivalent (Index Twin): mirror exact tradebook cash flows into a passive benchmark; computes Kaplan-Schoar PME, Direct Alpha, twin value, and net currency value added vs the index (see [below](#public-market-equivalent-index-twin)) |
| `portfolio remove <SYMBOL> [--yes]` | Remove a holding and its lots without recording a sale; asks first when it has more than one lot |
| `portfolio target` / `portfolio target show [--json]` / `portfolio target set <KEY> <PCT>` / `portfolio target remove <KEY>` / `portfolio target clear` / `portfolio target drift [<PCT>] [--json]` | Target weights by symbol (held or not yet held), sector, or asset class (`equity`, `debt`, `gold`, `international`, `cash`). A symbol target is carved out of its sector and asset-class targets, so nested targets never count twice; keys that match nothing are reported. `target clear` removes every target. `target drift` with no percentage shows current vs target per key with the 5/25 band (5 points or 25% of the target, whichever is smaller); with one, saves the drift tolerance band (the default is the profile's `default_drift_pct`, which `target show` prints) |
| `portfolio analyze [--quick] [--json\|--csv]` (or `portfolio dash`) | Dashboard of the whole portfolio in the base currency (see [below](#portfolio-analysis)). `--quick` skips the price history and sector lookups (no risk figures); `--csv` prints the holdings table (`--csv --groups`: the market, currency, cap-tier and sector tables), `--json` everything |
| `portfolio checkup [--offline] [--json\|--csv]` | Scan of the whole portfolio in six lenses, ranked by the money at stake: each finding says what was observed, the amount or weight at stake and the command to inspect it, with an Action column per holding (see [below](#portfolio-checkup)) |
| `portfolio review [--horizon short\|long] [--json]` | Each holding's [action](#actions), sorted EXIT, TRIM, ADD, HOLD, then NO CALL, with its reasons, invalidation price, review date, size hint and tax note (see [below](#portfolio-review)) |
| `portfolio wrap [--year YYYY \| --fy YYYY-YY] [--redact] [--offline] [--json]` | Year in review: return against the benchmark, top contributors and detractors, money deployed, dividends, realised gains, diversification, checkup pass count and a tax footer. `--redact` hides every amount (see [below](#portfolio-wrap)) |
| `portfolio xray [--json]` (or `portfolio x-ray`) | Portfolio X-Ray: look-through overlap across direct stocks, ETFs and mutual funds, combined effective company exposures, 10% concentration risk alerts, broad asset-class split, market-cap breakdown, and sector concentration |
| `watch` / `watch show [--json\|--csv]` / `watch add <SYMBOLS...> [--force]` / `watch remove <SYMBOLS...>` (`w`) | Watchlist |
| `alert [list] [--json\|--csv]` / `alert add <SYMBOL> <CONDITION> <NUMBER> [--period N] [--on-close] [--cross] [--force]` / `alert check [--json\|--csv]` / `alert remove <N>` | Price and technical alerts — conditions below; `alert list` shows each condition in words |
| `alert watch [--interval 60] [--once]` | Check alerts while their market is open and notify each trigger once (see [notifications](bots.md#notifications)). `--once` checks every alert once and exits, for cron |
| `calendar [SYMBOLS...] [--days 30] [--holdings\|--watchlist] [--json\|--csv]` | Upcoming results, ex-dividend and dividend payment dates, splits and bonus issues for your holdings and watchlist (or the named symbols), sorted by date, with today and the next 7 days marked. See [the calendar](#results-and-corporate-calendar) |
| `import <kite\|indmoney\|groww\|upstox\|cas\|csv> <FILE> [--replace] [--sheet NAME] [--account NAME] [--password PW]` | Import broker holdings, a lot list, a tradebook (CSV or `.xlsx`: one sheet is read, the first whose header fits, or `--sheet NAME`; Excel date cells included) or a CAS (casparser JSON/CSV); re-importing updates instead of doubling (see below and [brokers.md](brokers.md#importing-holdings)). `--account` names the demat account the file belongs to, so two accounts at one broker keep separate lots. `--password` opens a password-protected CAS PDF (needs casparser on `PATH`; else `STOCKWISE_CAS_PASSWORD` or a hidden prompt) |
| `export <portfolio\|watchlist>` | Write a CSV to the current directory |
| `wealth` | Save today's portfolio value and show growth (run daily) |
| `rebalance [--inflow N [--with-sells]] [--raise N] [--cash-buffer N] [--drift N] [--min-trade N] [--brokerage N] [--fx-spread PCT] [--json]` (or `portfolio rebalance`) | Whole-share trades back to targets (or equal weight) whose cash balances: sells pay for buys, and what is left goes to the holdings furthest below target. `--inflow` invests new cash without selling (`--with-sells` adds it to a full rebalance); `--raise` sells only, to raise that much cash at the lowest tax; `--cash-buffer` keeps cash uninvested. Each sell shows its lots, STCG/LTCG and tax; each trade its costs (see [below](#rebalance)) |
| `harvest [--carry-st N] [--carry-lt N] [--json\|--csv]` | Tax-loss and LTCG-exemption harvesting candidates, by FIFO sale in India and by lot in the US (see [below](#harvest)) |
| `tax <in\|us\|uk> [--json\|--csv]` | Capital-gains estimate for the current tax year: realized sales and unrealized lots, each with its own holding period (see below) |
| `tax export [in\|us] [--fy 2025-26] [--out FILE] [--json]` | Capital-gains schedule of a tax year's sales: an ITR Schedule CG / 112A CSV for India, a Form 8949 worksheet for the US (see [below](#tax-export)) |
| `tax export in [--schedule cg\|os\|fa\|form67] [--slab 30] [--withholding PCT] [--fy 2025-26] [--out FILE] [--json]` | India: Schedule OS (dividends and interest with TDS), Schedule FA Table A3 (foreign assets, calendar year) or Form 67 (foreign tax credit) instead of Schedule CG (see [below](#dividend-and-interest-income-schedule-os) and [Schedule FA](#foreign-assets-schedule-fa-and-form-67)) |
| `tax export in --audit-pack DIR [--fy 2025-26]` | One workpaper CSV per rupee sale (lot, dates, cost, 31 Jan 2018 FMV, gain, rule applied), `summary.csv` and the Schedule CG CSV in `DIR`, written whole (see [below](#audit-pack)) |
| `tax reconcile <FILE> [--broker zerodha\|groww\|upstox\|other] [--fy 2025-26] [--all-accounts] [--json\|--csv]` | Compare realised gains with a broker's tax P&L (CSV or `.xlsx`) sale by sale, with the likely reason for each difference and STCG/LTCG totals (see [below](#tax-reconcile)) |
| `tax income [--fy 2025-26] [--out FILE] [--withholding PCT] [--json\|--csv]` | Dividends and SGB interest with gross, TDS and net, by advance-tax period, for ITR Schedule OS (see [below](#dividend-and-interest-income-schedule-os)) |
| `tax foreign [--year 2025 \| --fy 2025-26] [--slab 30] [--withholding PCT] [--json]` | Schedule FA (initial, peak and closing values of foreign shares in the calendar year, in rupees) and Form 67 foreign tax credit inputs (see [below](#foreign-assets-schedule-fa-and-form-67)) |
| `tax advance [--fy 2025-26] [--paid [DATE:]AMOUNT]... [--record] [--slab PCT] [--json]` | Advance tax quarterly schedule (15 Jun, 15 Sep, 15 Dec, 15 Mar) under Indian Income-tax Act Section 208/211 with Section 234C(1) capital-gains proviso and interest estimator, and the next instalment due with the interest if it is missed (see [below](#advance-tax-india)) |
| `divcal` | Dividend ranking and income for portfolio + watchlist |
| `income [--months 12] [--by-holding] [--json\|--csv]` (or `portfolio income` / `portfolio dividends`) | 12-month trailing and projected dividend, coupon and passive-income calendar, Yield-on-Cost (YoC), and dividend payout safety checks (see [below](#passive-income-and-dividend-calendar)) |
| `income upcoming [--days 60] [--json\|--csv]` | Radar of upcoming ex-dividend and payment dates for held and watchlisted shares (see [below](#passive-income-and-dividend-calendar)) |
| `pcorr` | Correlation between holdings from the days both traded, diversification warnings (`N/A` when two holdings share too few days) |

### Lots, sales and tax

> **Guide:** [File your Indian capital-gains tax](guide-tax.md).

Every purchase is a lot: quantity, price, date and where it came from
(`manual`, `import:kite`, `broker:angel`, ...). `portfolio sell` takes shares
from the oldest lots first — FIFO, as Indian demat accounts and the IRS
default work — and keeps each slice as a realized sale.

`tax` classifies every lot by its own purchase date (long-term after more than
12 months for India and the US) and shows realized gains (sales this tax year)
separately from unrealized ones (if you sold everything today). For India it
applies the set-off rules: gains and losses of the same kind net first; a
short-term loss is set off against short-term gains, then long-term gains; a
long-term loss only against long-term gains; then ₹1.25 lakh of LTCG is exempt
and STCG is taxed at 20%, LTCG at 12.5%. Losses left over are shown as carry
forward. Holdings in another currency (e.g. US shares in `tax in`) are listed
but not included. It is an estimate — it includes the 4% cess but not surcharge or
brought-forward losses.

Grandfathering (India): for a long-term lot bought on or before 31 Jan 2018, the
cost is the higher of the actual cost and the lower of the 31 Jan 2018 price and
the sale price (Section 112A). `tax in`, `tax export in`, `tax advance` and
`harvest` all use it. The price is the day's highest for a listed share (Yahoo)
and the NAV for a mutual fund with an AMFI code; it is looked up once and cached.
A lot whose price cannot be found keeps its actual cost, and `tax export in`
says so in its notes. Prices are split-adjusted, so a lot recorded before a split
or bonus must carry its adjusted cost.

Same-day trades (India): a buy and sell of one share on the same day net
against each other first, in a tradebook import and in `trade sync` fills. The
matched quantity is recorded as an intraday sale (priced at that day's average
buy and sell) and the older lots are left alone. `tax in`, `tax export in` and
`tax advance` keep intraday results out of Sec. 111A/112A and show them
separately as speculative business income at your slab rate (losses can only
be set off against speculative gains). Without a date-time column a delivery
buy and sell of held shares on one day cannot be told apart from intraday.

Mutual funds and SGBs in `tax in`: equity-oriented funds — by their AMFI
category: equity schemes (ELSS included), arbitrage, aggressive hybrid and
equity savings funds, and Nifty/Sensex index funds — are counted like listed
shares and named under the estimate. Debt and other funds (taxed at your slab
rate for units bought from 1 Apr 2023, at 12.5% after 24 months for older
units), funds not found in AMFI's file, and SGBs (tax-free when RBI redeems
them; sold on the exchange, 12.5% after 12 months without the ₹1.25 lakh
exemption) are left out and listed with the rule that applies (`other_rules`
in `--json`). So are exchange-traded gold, silver and international ETFs (12.5%
after 12 months with no exemption, slab rate before) and liquid and debt ETFs
(slab rate at any holding period for units bought from 1 Apr 2023 under Sec.
50AA; units bought before that day are long-term after 12 months, see below).
StockWise tells them by category: the
underlying of each ETF in NSE's ETF list (`eq_etfseclist.csv`, cached for a day:
"Domestic Price of Gold", "NASDAQ 100 Index", "Nifty 1D Rate Index", a G-Sec or
Bharat Bond index, ...). Offline, or for an ETF not in that list, a built-in
table of NSE symbols (`GOLDBEES`, `SILVERBEES`, `LIQUIDBEES`, `MON100`, Bharat
Bond `EBBETF…`, ...) and ETF names that say gold, silver, liquid, gilt, G-Sec,
SDL, bond, Nasdaq or Hang Seng decide; an ETF none of these place is counted
like a listed share, as are equity ETFs such as `NIFTYBEES`. `harvest` skips the same holdings (they
are listed under "Not checked") and `tax export in` and `tax advance` leave
their sales out.

`tax in`, `harvest`, `tax export in` and `tax advance` share one rule for what
counts as a listed share or an equity-oriented fund (Sec. 111A/112A); the other
three load AMFI's file to read the category of any fund you sold. Crypto (such
as `BTC-INR`) is never in 111A/112A: it is taxed at a flat 30% under Sec.
115BBH with no set-off of losses, `tax export in` lists it under
`virtual_digital_assets`, and the other commands name it among the holdings
they leave out.

The year's sales outside 111A/112A get a section of their own in `tax in`
("Other capital gains", `other_heads` in `--json`), each with the rule that
applies:

- **Sec. 50AA** — debt-oriented funds and liquid, gilt and debt ETFs bought on
  or after 1 Apr 2023, and market-linked debentures whenever bought: always
  short-term, added to your income at the slab rate. Gold, silver and
  international funds bought from 1 Apr 2023 were "specified mutual funds" too
  until 31 Mar 2025; sold from 1 Apr 2025 they follow Sec. 112.
- **Sec. 112** — other non-equity funds, ETFs and SGBs sold on the exchange:
  long-term after 12 months (listed) or 24 months (unlisted; 36 before 23 Jul
  2024), taxed at 12.5% without indexation and without the ₹1.25 lakh exemption
  (20% with indexation for sales before 23 Jul 2024; indexation is not applied).
  Short-term gains go to the slab rate. A net short-term loss here, or a net
  Sec. 112 loss, is set off against the listed-share gains in the estimate
  above. Estimate limits: StockWise nets Sec. 112 gains and losses with each
  other first and does not set a Sec. 112A loss off against a positive Sec. 112
  gain, which the Act allows (Sec. 70/74).
- **Sec. 115BBH** — crypto: 30% on each gain, no set-off of any loss (not even
  against another crypto gain) and no carry-forward. The 1% TDS under Sec. 194S
  on the sale value is shown so you can match it with Form 26AS.

### Mutual funds and SGBs

> **Guide:** [Import your trades](guide-portfolio.md#3-import-mutual-funds-from-a-cas) shows how to load a CAS.

Holdings that Yahoo Finance doesn't quote are priced by StockWise itself, the
same way in `portfolio`, `dashboard`, `tui`, `serve`, `wealth`, `rebalance`,
`harvest`, `tax`, `portfolio sell` and `report portfolio`:

- **Mutual funds** — `MF:<AMFI code>` (`portfolio add MF:120503 10 50`) or
  `MF:<ISIN>` (the growth/payout or IDCW-reinvestment ISIN, as `import cas` and
  Groww imports store them) — at the latest NAV in AMFI's
  [NAVAll.txt](https://portal.amfiindia.com/spages/NAVAll.txt) (cached 12
  hours). NAVs are end of day: the table says "NAV as of &lt;date&gt;" for each fund,
  and `--json` rows carry `name`, `price_source: "amfi_nav"` and `nav_date`. A
  code or ISIN that isn't in AMFI's file, or a scheme AMFI lists without a NAV,
  stays N/A with a note (find codes with `mf search`) — never 0. A scheme whose
  last NAV is over a month older than the file is flagged as possibly matured.
  `MF:` symbols are never sent to Yahoo, and `watch`/`alert` see the NAV too.
- **Sovereign Gold Bonds** — the NSE symbol (`SGBNOV26`, stored as
  `SGBNOV26.NS`) or the tranche's ISIN — at the last price in NSE's SGB list
  (`price_source: "nse_sgb"`). When NSE refuses or shows no trade, the value is
  one gram of gold at the international spot price per unit, labelled
  "estimated from gold spot" (`gold_estimate`). Values leave out the 2.5% yearly
  interest, which is paid to your bank every six months.

Older versions stored `portfolio add MF:120503` as `MF:120503.NS` (and
`watch add` / `alert add` likewise); those symbols are repaired when the files
load.

### Passive income and dividend calendar

`stockwise income` (or `stockwise portfolio income`) is a 12-month trailing and
projected dividend, coupon and passive-income engine for dividend-growth
investors, retirees, and bond/SGB holders:

- **Monthly Cashflow Schedule:** Shows month-by-month cash flows across trailing
  12 months (received dividends recorded in `portfolio.dividends`) and forward N
  months (default 12), with a visual terminal bar chart and `--json`/`--csv`
  serialization.
- **Yield on Cost (YoC):** Calculates per-holding and per-lot Yield-on-Cost
  (`YoC = Annual Dividend / Original Lot Cost`), comparing it against current
  market yield to reveal dividend-growth compounding over time.
- **TDS and Withholding Tax:** Automatically computes withholding tax (flat 10% TDS
  for Indian dividends and 25% WHT for US foreign dividends; 0% assumed for other
  currencies) to report net spendable income.
- **Sovereign Gold Bonds & Fixed Income:** Models half-yearly 2.5% p.a. SGB
  coupons on issue nominal values across maturity cycles.
- **Payout Safety & Sustainability:** Evaluates earnings payout ratios,
  diagnosing distributions as `SAFE` (< 60%), `CAUTION` (60–85%), or `WARNING`
  (> 85% or negative earnings).
- **Upcoming Radar (`stockwise income upcoming`):** Checks upcoming ex-dividend
  and payment dates for holdings and watchlists, showing held share counts and
  calendar details.

### Harvest

> **Guide:** [File your Indian capital-gains tax](guide-tax.md#2-look-at-harvest-candidates-before-31-march).

`harvest` lists what you could sell to book a loss, and which long-term winners
you could sell to use this year's unused ₹1.25 lakh LTCG exemption. For India it
follows FIFO, as a demat account does: to reach a losing lot you must sell the
older lots first, so the row shows that whole sale (`shares`, and the net `st_pnl`
and `lt_pnl` it books). If selling up to a losing lot books a net gain in all, the
holding is not offered as a loss; `fifo_blocked` (and a note on screen) says how
many shares you would sell and what the sale books. For the US you can choose lots,
so only the losing lots are offered.

For the US the row's `lots` names the lots sold, and the screen prints the
command that records the sale exactly (`portfolio sell AAPL 8 --lots 2`), so the
ledger books the same loss as the broker. For India a note says to buy back on
the next trading day: a same-day sell and buy nets out as an intraday trade.

The LTCG step-up row is the exact share count, oldest lots first and long-term
lots only, whose gain fits in what is left of the exemption (`tax_free_gain_room`:
the exemption plus any losses that would set off against the gain first), and
`estimated_tax_saved` is that gain at 13% (12.5% plus 4% cess). `--carry-st` and
`--carry-lt` add capital losses brought forward from earlier years; they set off
like this year's losses and are echoed in `carried_forward`.

For the US, `harvest -m us` also checks the 30-day wash-sale rule (IRS
Publication 550): a loss is disallowed when the same shares are bought within 30
days before or after the sale. A loss candidate with a purchase in the last 30
days that the sale would not sell (a lot at a gain, or shares already bought and
sold; bonus shares do not count) is flagged, with that date in `recent_buy`. `wash_sale_window_end` is the
last day of the window for a sale made today: a buy of the same shares on or
before it would make the loss a wash sale. `estimated_tax_saved` does not subtract
a disallowed loss.

`estimated_tax_saved` is how much the tax estimate falls when the loss is booked
on top of this year's realized gains, so set-off order, the exemption and cess are
counted; a loss with nothing to offset saves 0 and is carried forward. The
remaining exemption is shared: each winner takes its part of it, biggest first,
and the saving counts only that part. Holdings in another currency (US shares for
`in`) are listed under `other_currency` and not counted; holdings taxed under
other rules (crypto, debt funds, SGBs, gold and similar ETFs) under `other_rules`.

### Rebalance

> **Guide:** [Rebalance to target weights](guide-rebalance.md).

`rebalance` turns targets into whole-share orders (fund units to three decimals).
Holdings without a price or exchange rate are left out with a warning, and the
weights are of the rest. A target for a symbol not held yet gives a buy line
(`held: false`).

- **Cash balances.** Sells come first; buys are paid from them plus any
  `--inflow`, less `--cash-buffer`. Short of cash, holdings over target inside the
  drift band are trimmed, then the lowest-priority buys; cash left over buys more
  of the holdings furthest below target while a share brings one closer.
  `net_cash` is what the trades free (positive) or need (negative) after costs;
  `cash_left` is what stays uninvested. `--inflow` alone never sells, and splits
  the cash the way that minimizes the squared deviation from target.
- **Projected weights** (`projected_pct`) come from the rounded orders, of the
  holdings plus the cash left. The profile's `max_stock_weight` is a hard limit
  for single stocks (not ETFs, funds or gold bonds): a target above it is capped
  and the excess goes to the other holdings.
- **Lots and tax.** Indian shares and funds sell oldest first. Elsewhere a sell
  takes losing lots first, then the lots with the lowest tax, and leaves lots that
  turn long-term within 45 days for last; `record` gives the `portfolio sell
  --lots` command. Each sell has its `st_gain`, `lt_gain` and `tax` (added to this
  tax year's estimate, cess included); `tax.saved_vs_fifo` is what the lot choice
  saves. Gains taxed under other rules (an Indian resident's foreign shares, debt
  funds) are shown but not taxed (`untaxed_gain`).
- **Costs.** NSE/BSE trades: STT, stamp duty, exchange and SEBI fees with GST,
  and the DP charge on sells; Indian fund purchases: stamp duty; trades in
  another currency: an FX spread (`--fx-spread`, 0.5% by default); `--brokerage`
  per share order. Orders under `--min-trade` (₹1,000 / $20 by default) are left
  out.
- **`--raise N`** sells only, lowest tax first (losses, then gains inside the
  exemption, then long-term, then short-term), most overweight first among equals.

### Tax export

> **Guide:** [File your Indian capital-gains tax](guide-tax.md#4-export-the-schedule-for-your-itr).

`tax export in` writes the realized sales of an Indian financial year as a
spreadsheet laid out for ITR Schedule CG and Schedule 112A: one row per lot
sold with section (111A short-term, 112A long-term), ISIN, scrip, quantity,
sale date and value, purchase date and cost, days held and the gain, then a
summary block — gains and losses of each kind, the short-term loss set off
against LTCG, the ₹1.25 lakh LTCG exemption, taxable STCG/LTCG, the estimated
tax and losses to carry forward. ISINs come from NSE's equity list (shares)
and AMFI's NAV file (funds imported from a CAS), both cached for 12 hours;
one that can't be found is left blank and named. `tax export us` writes a
Form 8949 worksheet instead: Part I (held one year or less) and Part II rows
with description, dates (MM/DD/YYYY), proceeds, cost basis and gain, then the
Schedule D totals, the $3,000 loss deduction and the carryover.

Wash sales (IRS Publication 550) are found from your StockWise records: a sale
at a loss is one when shares of the same symbol were bought within 30 days
before or after it, counting open lots and the buy side of other sales, but not
the rest of the lot sold or shares sold on or before the day of the loss. The row gets code `W` in
column (f) and the disallowed loss as a positive amount in column (g); column
(h) is (d) − (e) + (g). A partial replacement disallows only its share of the
loss (replacement shares over shares sold), and each purchase replaces one loss
only. Replacement shares take on the disallowed loss and the holding period of
the shares sold, share by share: when they are sold, their row adds the loss to
the basis (column e) and the days to the holding period (which decides Part I
or II), and a sale of which only some shares were replacements is split into
one row per part. A note names replacement shares still held. The Schedule D
totals include the adjustments. A replacement with an estimated purchase date
is named in a note, as that wash sale may not be one. Lots bought on the same
day, in the same account and at the same price as the lot sold count as the same
purchase, so they are not replacements. Bonus shares are not purchases. Only USD
stocks are checked (crypto such as `BTC-USD` is left out); row amounts are split
in whole cents, so the rows add up to the Schedule D totals. Purchases in accounts StockWise does not know about (an
IRA, a spouse's account) are not seen.

```bash
stockwise tax export in --fy 2025-26 --out cg-2025-26.csv   # file + summary on screen
stockwise tax export in --fy AY2026-27 > cg.csv            # same year, by assessment year
stockwise tax export us --fy 2025 --json | jq '.summary'
```

`--fy` takes `2025-26`, `2025-2026` or `AY2026-27` (India) and `2025` (US);
the default is the current tax year. Without `--out` the CSV (or `--json`)
goes to stdout. It is an **estimate — verify it with your broker's tax P&L**:
it only knows sales StockWise recorded (`portfolio sell`, tradebook and CAS
imports) at their recorded prices, without brokerage or STT. Shares bought on
or before 31 Jan 2018 use the Section 112A grandfathered cost (see above); sales
in another currency (US shares for `in`) and crypto (taxed at 30% under section
115BBH, Schedule VDA) are listed but left out, and so are debt and other funds
and SGBs (see `tax in` and the audit pack for their rules).

The rates follow the **sale date**: sales before 23 Jul 2024 are taxed at 15%
(STCG) and 10% (LTCG), later ones at 20% and 12.5%. The LTCG exemption is ₹1 lakh
up to FY 2023-24 and ₹1.25 lakh from FY 2024-25, for the whole of that year. In a
mixed FY 2024-25, losses and the exemption are used against the higher-rate gains
first (the lowest tax; the Act does not prescribe an order), and the summary
shows how much of the taxable gain came from before 23 Jul 2024.

### Audit pack

`tax export in --audit-pack DIR` writes a working paper for every rupee sale of
the financial year, whatever rule taxes it, so you (or your CA) can check each
gain by hand:

- `sale-001-HDFCBANK-2026-07-20.csv`, one per sale, numbered in sale order: a
  `field,value` list with the symbol, ISIN, account, the lot's source, quantity,
  purchase and sale dates (and whether the purchase date is estimated), days
  held, term, purchase price, actual cost, the 31 Jan 2018 fair market value per
  unit and in total where grandfathering applies, the cost used and how it was
  chosen, sale price and value, the gain, the rule applied (Sec. 111A, 112A with
  or without grandfathering, 50AA, 112, 115BBH or Sec. 43(5) intraday) and the
  computation in words.
- `summary.csv`: the same columns, one line per sale, then the totals by rule.
- `schedule-cg.csv`: the Schedule CG / 112A export of `tax export in`.

Amounts are worked in paise, so each workpaper's equation balances to the
paisa. The cost of a grandfathered lot is shown with its figures, `max(actual
cost, min(31 Jan 2018 FMV, sale value))`. An intraday sale's gain is net of its
recorded charges (business income, as in the Schedule CG export's intraday
total); capital gains are before charges. Same-day crypto sales stay under Sec.
115BBH. Sales in another currency are not in the pack. The pack is written to
a staging folder and moved into place only when every file is written: a rerun
into the same folder replaces the earlier pack whole, and a folder holding
anything else is refused.

```bash
stockwise tax export in --fy 2026-27 --audit-pack ~/tax/2026-27-audit
```

### Tax reconcile

> **Guide:** [File your Indian capital-gains tax](guide-tax.md#5-reconcile-with-your-brokers-tax-pl).

`tax reconcile FILE` reads a broker's tax P&L or capital-gains report —
Zerodha Console's tax P&L, Groww's capital gains report, Upstox's capital gain
statement, as CSV or `.xlsx` (every sheet) — and matches it with the sales
StockWise recorded, sale by sale. Columns are found by name (Symbol / Scrip
name / Stock name, ISIN, Quantity, Buy or Entry date, Sell or Exit date, Buy
and Sell value or rate, Profit / Realised P&L, Fair Market Value, Taxable
Profit, the charge columns, Short/Long term); section headings such as
"Equity - Short Term" set the term when there is no term column. The broker is
detected from the file, or given with `--broker`.

Matching uses the ISIN when both sides have one, else the NSE symbol, and the
sale date: first with the same purchase date, then on the sale date alone with
lots summed (the two sides may split a sale into lots differently). Each
matched sale that differs gets a likely reason:

| Reason | Usually means |
|---|---|
| quantity differs | a missing buy lot, or a split/bonus not applied (`portfolio actions --apply`) |
| buy date differs | FIFO across accounts, a transfer-in, an estimated purchase date |
| grandfathering | a pre-2018 lot: one side uses the 31 Jan 2018 FMV as cost |
| charges | the broker nets STT, brokerage and stamp duty |
| buy cost / sale value differs | a different price recorded |
| short/long-term differs | the holding period is counted differently |

Sales on one side only are listed, and the totals compare STCG, LTCG and
intraday on both sides (a broker row matched by ISIN is totalled under the
StockWise ticker). Cost, sale value and gain are each compared, so equal gains
over a different cost and sale value still differ. An exact-date group that
does not cover the whole quantity waits for the sale-date pass. Terms are
compared by quantity per term, whatever the row order. A row with no readable
profit, taxable profit or buy and sell value is listed as not compared, never
read as a zero gain. Each sheet of a workbook is read on its own. A company
name with no ISIN (some Groww rows) is not matched to a ticker: it shows on
both sides. For a Zerodha, Groww or Upstox file, StockWise compares its sales
from that broker's account plus those saved without an account;
`--all-accounts` compares them all; a file from another broker has no account
filter. Without `--fy` the year is the one most of the file's sales fall in.
`--json` and `--csv` give the results for scripts. Read-only: the portfolio is
not changed.

```bash
stockwise tax reconcile ~/Downloads/taxpnl-AB0000-2026_2027.xlsx
stockwise tax reconcile groww-capital-gains.csv --fy 2026-27 --json | jq '.by_symbol'
```

### Dividend and interest income (Schedule OS)

`tax income` lists a financial year's dividends — those `portfolio actions
--apply` recorded from your holdings' corporate actions, dated by ex-date —
and Sovereign Gold Bond interest (2.5% a year on the issue price, paid every
six months from the issue date on the units held that day). For each: gross,
TDS and net, and the ITR period it falls in (up to 15 Jun, 16 Jun–15 Sep, 16
Sep–15 Dec, 16 Dec–15 Mar, 16 Mar–31 Mar), which is also the Sec. 234C split
(a dividend received after a due date carries no interest for the earlier
instalments).

- **TDS** (estimate): once a company's or fund's dividends to you in the year
  pass ₹10,000 (₹5,000 up to FY 2024-25), 10% of all of them, under Sec.
  194/194K. A Form 15G/H, a lower-deduction certificate or a missing PAN
  changes it: match it with Form 26AS / AIS.
- **SGB interest**: no TDS.
- **Foreign dividends**: converted to rupees at the day's ECB reference rate
  (an amount recorded in pence is converted as pounds). The tax withheld abroad
  is not recorded: `--withholding 25` (or the rate on your statement) shows it;
  without it the withholding is "unknown".
- **Dates**: a dividend is placed by its ex-date, as the payment date is not
  recorded; one paid after a period's end can sit in the earlier period.

When a dividend has no rupee rate or an SGB tranche is not known, it is left
out and named, and the CSV ends with an `incomplete` block listing them.

`--csv` (or `tax export in --schedule os`) prints the Schedule OS CSV: one row
per dividend or coupon, then the quarterly breakup. `--out FILE` also writes
it while showing the table. `tax advance --slab 30` adds this income to the
advance-tax schedule at your slab rate: dividends in their quarter, SGB
interest in every quarter (it has no Sec. 234C proviso).

### Foreign assets (Schedule FA and Form 67)

`tax foreign` is for residents holding foreign shares (US stocks, RSUs):

- **Schedule FA, Table A3**, for the **calendar year** (`--year 2025`, or the
  calendar year in which `--fy` starts; ITR for FY 2025-26 reports 2025): one
  row per purchase lot held at any time in the year, with country, entity,
  date acquired, initial value (cost at the purchase day's rate), peak value
  (the highest of units held × close × that day's rate over the year's trading
  days), closing value on 31 December, dividends credited and sale proceeds.
- **Form 67**: each foreign dividend of the financial year, converted at the
  rate of the last day of the month before (Rule 128), with the tax withheld
  abroad at the `--withholding` rate from your broker's statement and the
  credit — the lower of that tax and the Indian tax on the same income at
  `--slab` (default 30%, 0 to 50) plus cess. Without `--withholding` the tax
  and the credit are "unknown". Dates are ex-dates (`date_basis` column), as
  payment and withholding dates are not recorded. For a return under Sec.
  139(1) or 139(4), Form 67 is due by the end of the assessment year (Rule
  128(9)); for an updated return, by the day it is filed.

A purchase made with rupees at a recorded rate (`portfolio add --currency
INR`) uses that rate for its initial value. A value missing a rupee rate for
some day is marked in `missing_rates` (not shown as zero), holdings left out
are listed, and the CSVs end with an `incomplete` block. `--year` must be a
calendar year whose financial year (from April) has started.

Rupee values use the ECB reference rate for the day (Frankfurter); the rules
ask for the SBI telegraphic-transfer buying rate, which differs slightly.
`tax export in --schedule fa` and `--schedule form67` write the CSVs.

```bash
stockwise tax foreign --year 2025
stockwise tax export in --schedule fa --fy 2025-26 --out schedule-fa-2025.csv
```

### Advance tax (India)

> **Guide:** [File your Indian capital-gains tax](guide-tax.md#3-check-advance-tax).

Under Section 208 and 211 of the Indian Income-tax Act, any taxpayer whose
estimated tax liability for the financial year is ₹10,000 or more must pay
advance tax in four quarterly instalments:
- **By 15 June**: At least 15% of estimated tax
- **By 15 September**: At least 45% of estimated tax
- **By 15 December**: At least 75% of estimated tax
- **By 15 March**: 100% of estimated tax (capital gains realized between 16 and 31 March are payable by 31 March)

Because capital gains cannot always be predicted in advance, the **proviso to
Section 234C(1)** specifies that deferment interest is not levied if the shortfall
is due to capital gains realized after an instalment date, provided the remaining
tax is paid in subsequent instalments. `stockwise tax advance` groups realized sales
into the four statutory quarterly cut-offs and calculates cumulative set-offs under
Indian equity rules, by sale date: from 23 July 2024 STCG @ 20% and LTCG @ 12.5%, before it 15% and 10%; the LTCG exemption is ₹1.25L (₹1L up to FY 2023-24):

```bash
stockwise tax advance                             # current FY advance tax schedule
stockwise tax advance --fy 2025-26                # specific financial year
stockwise tax advance --paid 2025-06-14:3000 --paid 2026-01-10:17800   # dated payments: exact Section 234C interest
stockwise tax advance --paid 25000                # a total with no date: assumed paid on time
stockwise tax advance --json                      # programmatic JSON output
```

- **Section 208 Check**: Automatically flags whether advance tax is mandatory (total liability ≥ ₹10,000) or exempt.
- **Status**: A quarter is `Paid` only when payments dated on or before its due date cover the minimum. With no `--paid`, a quarter past its due date is `Overdue`, because StockWise has no record of any payment.
- **Section 234C Deferment Interest**: Section 234C looks at what was paid by each due date, so pass each payment with its date (`--paid 2025-06-14:3000`, repeated). It then charges 1% a month (3% for Q1–Q3, 1% for Q4) on each shortfall, with the shortfall rounded down to a multiple of ₹100 first (Rule 119A: a ₹1,000 shortfall at 3% is ₹30), when the payments by that date are below the safe-harbour level (12% in Q1, 36% in Q2, 75% in Q3, 100% in Q4). The capital-gains and dividend proviso holds only if their tax is paid in the later instalments or by 31 March: once the year is over and the payments by 31 March fall short, every instalment is measured against the whole year's tax. Interest income (SGB coupons) has no proviso and counts toward every instalment. For Q4 the requirement on 15 March is the tax on gains up to 15 March; tax on gains after it carries no interest when paid by 31 March (the proviso). A payment dated before the financial year began is ignored (it was made for an earlier year) and the output says so. A payment without a date is assumed to have been made before every due date, which can understate the interest.
- **Cess**: The tax and the instalments include the 4% health and education cess; surcharge is left out.
- **Section 234B Note**: Flags potential Section 234B liability if assessed after 31 March and cumulative advance tax paid is below 90% of total liability.
- **Next instalment**: The next due date with the cumulative amount required on it (for 15 March, the tax on gains and dividends up to 15 March), what has been paid up to today (dated payments after today are not counted yet), what is left, and the Section 234C interest if nothing more is paid: 3% (Q1–Q3) or 1% (Q4) of the shortfall rounded down to a multiple of ₹100 — none when the payments already meet the safe harbour (12% by 15 June, 36% by 15 September). Under ₹10,000 of tax for the year nothing is shown as payable. `next_instalment` in `--json`.
- **Recorded payments**: `--paid 2026-12-10:25000 --record` saves dated payments in `advance_tax.json` in the data directory, each for one financial year (`--fy`, or the year of its date; a late payment recorded with `--fy 2025-26` counts for that year). Later runs for that year count them without `--paid`; `--paid` without `--record` counts for the year asked for, this run only. An unreadable `advance_tax.json` is an error and is left untouched.
- **Dividends and interest**: `--slab 30` adds the year's dividends and SGB interest (see `tax income`) at your slab rate, each in the quarter it was received.
- **Reminder**: notifiers can call the advance-tax reminder hook, which fires within 7 days of a due date when something is still to pay. It covers capital gains and recorded payments only: tax on dividends and SGB interest is not in it, and a portfolio with income but no sales gets no reminder.


### Plain-text accounting export

`portfolio export --format beancount|hledger [-o, --output FILE]` writes the active portfolio as
a double-entry journal for [beancount](https://beancount.github.io/) or
[hledger](https://hledger.org/), to standard output or to `-o FILE`:

```bash
stockwise portfolio export --format beancount -o portfolio.beancount
bean-check portfolio.beancount
stockwise portfolio export --format hledger > portfolio.journal
hledger -f portfolio.journal check
stockwise --portfolio us portfolio export --format hledger   # a named portfolio
```

The journal is rebuilt from what StockWise records, in date order:

- `commodity` declarations for each currency and holding, and `open` /
  `account` declarations for each account used.
- A purchase for every open lot and for every lot behind a recorded sale,
  into `Assets:Investments:<Account>` (the demat account, such as
  `Zerodha`, or `Default`), paid from `Assets:Cash:<CUR>`. In beancount the
  lot carries its cost (`{2400.5 INR}`).
- A sale for every realized lot, at its matched cost basis (beancount:
  `{cost, buy date} @ sale price`; hledger: `@ cost` with the lot date and sale
  price in a comment, because hledger balances at the `@` price), with the gain
  on `Income:CapitalGains:ShortTerm` or `Income:CapitalGains:LongTerm`. The
  split follows the listed-share holding period of the price currency's
  country (more than 12 months for INR and USD; the UK has no split, so GBP
  gains are short-term); same-day trades are short-term.
- A dividend for every recorded dividend, into `Income:Dividends`. An
  `Expenses:Tax:Withholding` leg is added when a record holds the tax
  withheld; StockWise's dividend records hold the gross amount only, so it does
  not appear yet.

Commodity names follow beancount's rules (capital letters, digits and
`'._-`; starting with a letter, ending with a letter or digit; at most 24
characters). The symbol is upper-cased and any other character becomes `_`:

| Symbol | Commodity |
|--------|-----------|
| `RELIANCE.NS` | `RELIANCE.NS` |
| `M&M.NS` | `M_M.NS` |
| `MF:120503` | `MF_120503` |
| `^NSEI` | `X_NSEI` (an `X` in front when the name does not start with a letter) |
| `9988.HK` | `X9988.HK` |

A longer name is cut and ends in a short hash, and two symbols that map to
the same name get `-2`, `-3`, ... The original symbol is kept in the
`commodity` directive's `name` metadata (beancount) or comment (hledger), and
in each transaction's `symbol` metadata or tag. hledger needs names with
anything other than letters in double quotes (`"RELIANCE.NS"`).

Quantities and prices are split-adjusted, as StockWise stores them; London
prices in pence (`GBp`) are converted to pounds. Amounts are exact decimals,
so every transaction balances to zero. Symbols with no exchange suffix are
booked in the `-m` market's currency. The journal is for records and
education; check it against your broker's statements.

### Portfolio analysis

`stockwise portfolio analyze` prints a dashboard of every holding:

- **Summary:** value, P&L, today's move, cost and the *effective number of
  holdings* (1 / the sum of squared weights: 10 equal holdings give 10).
- **Holdings:** weight, value in the base currency, P&L %, today's move, the
  tax term (`LT`; `ST 17d` until the lot turns long-term; `mix` for both),
  distance from the 200-day average, a 52-week range strip (`●` marks the
  price), yearly volatility, the long-term score and how many observations
  name it (`●` when one is a notice).
- **By market, currency, cap tier and sector:** value, weight and P&L in the
  base currency; a market or currency group also shows its total in its own
  currency (pence shown as pounds).
- **Short-term and long-term:** unrealised gains by tax term (12 months for
  Indian and US equity), lots on each side, the next lots to turn long-term,
  and the estimated tax if everything were sold today at the standard rates
  of your market (before this year's other gains and losses).
- **Risk and concentration:** the yearly volatility and largest fall of the
  portfolio as it is weighted today (from a year of daily closes), its beta
  against the main index, the share of the value with price history behind
  it, the value-weighted long-term score, the largest holding and the top five.
- **Observations:** readings worth a look, each with its figures: a stock or
  small caps above your investor profile's limits, a sector above 30%, a
  holding more than 10% below cost (with the loss), a lot turning long-term
  within 45 days (with the tax difference at the standard rates), a price
  25% below its year high, 5% below its 200-day average or at a 52-week high
  or low, a weak long-term score, an estimated purchase date, money in other
  currencies, and holdings left out of the totals.

These are readings of the data. The dashboard gives no action; for each
holding's action, run [`portfolio review`](#portfolio-review). `--json` prints
every figure. The TUI shows the same
analysis on its own screen (key `p`).

### Portfolio checkup

`stockwise portfolio checkup` reads the active portfolio and reports
findings in six lenses:

1. **Concentration, overlap and cap tiers**: a single company above the
   profile's `max_stock_weight` (12% unless set), counting shares held through
   funds whose holdings are known; small and micro caps together above
   `max_smallcap_weight`; two funds sharing more than 35% of their holdings;
   fewer than 5 effective bets (1 / HHI of the look-through weights).
2. **Forensic and fundamental flags**: Piotroski F-Score of 3 or less (with
   at least 7 of 9 signals evaluated), Altman Z in the distress zone,
   operating cash flow below 0.7× net profit, promoter shares pledged, a
   promoter stake down a point or more in a quarter, and a latest quarter with
   both net profit and operating margin below the same quarter a year ago.
3. **Valuation stretch**: P/E above 60 or PEG above 2.5 while revenue growth
   is slowing.
4. **Downside**: portfolio beta above 1.2, a fall of more than 25% from the
   peak of the last year, and pairs of holdings (each 2% or more) whose daily
   returns correlate above 0.75.
5. **Fees and tax**: the unused long-term exemption (India's ₹1.25 lakh under
   Sec. 112A) when long-term gains could use it, harvestable losses, the yearly
   cost of Regular-plan funds over their Direct twin, and lots turning
   long-term within 45 days.
6. **Goals and targets**: weights outside the `portfolio target` drift band
   and saved goals still short of their target.

Each finding states what was observed, the amount or weight at stake, and the
command that shows the detail (`stockwise score TCS.NS --explain`,
`stockwise harvest`, `stockwise mf bleed`, ...). Findings are ranked by the
value of the position (or the amount itself, for portfolio-wide readings)
times a severity weight of 1, 2 or 3. The summary line counts the checks that
ran and how many raised nothing.

Statements are read for the 30 largest stocks, four at a time; a source that
fails or times out is listed under "Not checked" and its checks are skipped.
`--offline` reads only the cache: prices from stored market snapshots
(`cache snapshots`), closes from stored daily bars, and saved fund
disclosures. It skips statements and Regular-plan costs. `--json` prints
`{base_currency, total_value, herfindahl, effective_bets, findings: [{lens,
rule, severity, symbol, observed, exposure, at_stake, weight_pct, inspect,
impact}], checks: [{rule, lens, clean}], clean, evaluated, skipped,
disclaimer}`; `--csv` prints one row per finding in rank order. The MCP tool
`portfolio_checkup` returns the JSON. Amounts are ranked in the base
currency: tax, fund and goal amounts are converted at today's rate, and a
source without a rate is listed under "Not checked". A check only counts as
run when its data was there: unknown cap tiers, funds without disclosed
holdings or a missing revenue trend leave the check out of the count.

The findings are observations. The Action column next to each holding comes
from the action engine, as in [`portfolio review`](#portfolio-review); each
JSON holding carries its `action` object.

### Portfolio review

`stockwise portfolio review [--horizon short|long] [--json]` gives each
holding its [action](#actions) and sorts them by what needs doing first:
EXIT, TRIM, ADD, HOLD, then NO CALL. Each holding shows:

- the action line: action, confidence, invalidation price and review date;
- up to three reasons;
- a size hint. For TRIM it gives the shares to sell to get back under your
  profile's `max_stock_weight` (half of it for a small cap); for ADD, the most
  the rules allow;
- a tax note when a lot turns long-term within 30 days, such as "waiting 23
  days saves about ₹4,120 in tax".

For a holding, the invalidation level trails: it is measured from the highest
close of the last 20 sessions. A close below it is an EXIT. The review works
from quotes, so it uses the long-term score as the composite and, without a
results date, reviews in 90 days on the long horizon. Funds, ETFs and SGBs
get NO CALL. `--json` prints one entry per holding, each with its `action`
object. The MCP tool `portfolio_review` returns
the same. Rules: [methodology.md](methodology.md#actions-methodology-1).

### Portfolio wrap

`stockwise portfolio wrap` prints a one-screen review of a year: the return
against the market's index (NIFTY 50, S&P 500, ...) and the difference in
basis points, the top three contributors and detractors, the money deployed,
sale proceeds, dividends and realised gains, the cap-tier split, the top
three sectors, the effective number of bets, how many checkup guardrails are
clean, and a tax footer (days left in the tax year, the long-term exemption
still unused, and for India the next advance-tax date).

The period is `--year 2026` (calendar year) or `--fy 2026-27` (April to
March). Without either, it is the current Indian financial year when the
portfolio totals in rupees, and the current calendar year otherwise. A year
still running is reviewed up to today.

The return is a Modified Dietz return over the period: the gain (end value
less start value less money added, net of sales and dividends) over the start
value plus the money added, each flow weighted by the time it was invested.
It is not annualised. The start values use the close of the day before the
period; mutual funds use their NAV history. Amounts in other currencies are
converted at today's rate. Holdings without a price at either end are named
and left out of the return.

`--redact` replaces every amount with a percentage: the cash flows become a
share of the money at work, contributors show their share of the year's
gains and losses, and the tax footer shows the share of the exemption used.
The card can then be shared without showing what the portfolio is worth.
`--json` prints the card, with the amounts set to null when redacted.

### Benchmark and attribution

> **Guide:** [Import your trades and see true returns](guide-portfolio.md#6-compare-with-the-index-and-look-through-your-funds).

`portfolio benchmark` rebuilds what you held at each close of the period
from your lots and sales (in today's share count, so splits don't break the
history), and compares the portfolio's time-weighted return with an index
over the same days.

```bash
stockwise portfolio benchmark                  # vs NIFTY 50 over 1 year (S&P 500 with -m us)
stockwise portfolio benchmark SENSEX -p 2y
stockwise portfolio benchmark SP500 --base inr # S&P 500 converted to rupees day by day
stockwise portfolio benchmark NIFTYBEES -p 6mo --csv
```

- Benchmarks by name: `NIFTY`, `SENSEX`, `BANKNIFTY`, `NIFTY500`, `SP500`,
  `NASDAQ`, `NASDAQ100`, `DOW`; or an index symbol (`^NSEI`) or any ticker.
  Periods: `1mo`, `3mo`, `6mo`, `ytd`, `1y` (default), `2y`, `5y`, `10y`, `max`.
- The period starts at the first close where something was held. Returns come
  from daily closes: a purchase or sale counts at that day's close, so the
  figure measures the holdings, not your fill prices. Dividends are left out
  on both sides (NIFTY 50 and the S&P 500 on Yahoo are price indices).
- Alpha (annualized, risk-free rate 0), beta, tracking error, information
  ratio and correlation come from the daily returns and need 20 of them.
- Attribution: each holding's average weight, its return while held, its
  contribution to the portfolio return and to the return over the index, in
  percentage points (linked with Carino's method, so they add up exactly to
  the totals); the same by sector, from Yahoo's asset profile ("Unknown" when
  Yahoo has none, "Mutual fund" for funds imported from a CAS, valued from
  their NAV history).
- Holdings in another currency are converted into the base currency (`--base`,
  else the holdings' one currency, else the market's) at each day's exchange
  rate; an index in another currency is converted too.
- Flagged under *Partial history*: holdings with no price history (left out),
  days held before a holding's price history starts (left out of those days),
  and purchases inside the period with an estimated date (the shares may have
  been held earlier than the history shows).

### Public Market Equivalent (Index Twin)

`stockwise portfolio pme` mirrors your exact tradebook cash flows into a passive benchmark index or ETF (e.g. `SPY`, `VOO`, `VWCE`, `NIFTY`, `NIFTYBEES`) on their matching trade dates:
- Every buy of \$X on date D buys \$X worth of the benchmark twin.
- Every sell on date D distributes the equivalent proceeds from the benchmark twin.
- Recorded dividends are distributed from the twin on matching dates.

It answers: *"Did my active trading and stock picking actually beat investing in a passive index on the exact same dates?"*

Metrics reported:
- **Index Twin Value**: Current value of the synthetic benchmark portfolio.
- **Value Added / Lost**: Terminal portfolio value minus Index Twin value (in exact currency units: +\$ / +₹).
- **Kaplan-Schoar PME (KS-PME)**: Compounded future value of distributions + terminal portfolio value over future value of investments (> 1.000 indicates active outperformance; 1.000 is parity).
- **Direct Alpha (Gredil, Griffiths, Stucke 2014)**: Annualized IRR of benchmark-discounted cash flows, yielding 0.00% on parity.
- **Holding Breakdown**: Direct Alpha and Value Added breakdown for each held asset.

XIRR (in `portfolio` and `portfolio lots`) is the money-weighted annual return
from each lot's purchase, each sale, recorded dividends and today's value. It
shows `—` with under 30 days of history. The portfolio figure includes closed
positions and converts other currencies at today's rate.

Lots marked `*` have an estimated date: holdings from before lot tracking
(dated when they were added to StockWise) or imported from a file without
dates. Fix one with `portfolio lots SYMBOL --lot N --date YYYY-MM-DD`.

**Corporate actions.** `portfolio actions` lists splits, bonus issues (Yahoo
reports a 1:1 bonus as a 2:1 split) and dividends since each holding's first
purchase. Nothing changes until `--apply`, which multiplies the quantity and
divides the price of lots bought before the split (cost and date unchanged)
and records each dividend on the shares held at its ex-date. Each lot
remembers the splits applied to it, so running it again is a no-op.

Indian tax treats bonus shares as a new purchase at zero cost on the
allotment date (Sec. 55(2)(aa)(iiia)), and the original shares keep their
cost and date. So for a `.NS` or `.BO` stock, StockWise looks each split up in
NSE's corporate-action filings. When NSE filed it as a bonus, `--apply` leaves
the older lots as they are and adds the bonus shares, `qty × (ratio − 1)`, as a
new lot at price 0 with source `bonus`, dated on the allotment date (NSE's
ex-date, since its feed gives no allotment date). FIFO then sells the older
shares first. For example, 100 shares bought at ₹1,000 plus a 1:1 bonus become
two lots — 100 @ ₹1,000 and 100 @ ₹0 — and selling 100 at ₹600 records a
₹40,000 loss on the older lot. A real split, and any split of a non-Indian
stock, still multiplies the lots. When NSE cannot be reached, splits are
applied as splits and the output says so: check any bonus lots among them
against your tax statement.

Indian demergers come from a table in StockWise, not Yahoo: Reliance → Jio
Financial Services (record date 20 Jul 2023, 1 for 1, 4.68% of cost), ITC →
ITC Hotels (6 Jan 2025, 1 for 10, 13.51%) and Tata Motors (`TATAMOTORS` or
`TMPV`) → `TMCV` (14 Oct 2025, 1 for 1, 31.15%). Each figure is from the
company's own filing with the exchanges. For every lot bought before the record
date, `--apply` adds a lot of the new company with the same purchase date
(Sec. 2(42A)) and that share of the lot's cost and charges, rounded to the
paisa (Sec. 49(2C)); the parent lot keeps its quantity and the rest of the cost
(Sec. 49(2D)). The new shares count as held only from the record date, so they
cannot be sold, or counted for a dividend, before it. Shares sold after the
record date but before `--apply` received the new shares too: their sale's cost
is split the same way and their new shares are added. Splits and bonuses before
the record date are applied first, so the entitlement is the record date's
share count. The company issues whole shares per demat account and pays for
fractions in cash, so each account's new shares are rounded down once across
`.NS` and `.BO`; the cash is not recorded. New-company shares already in an
account with no tradebook, broker order or `portfolio add` behind them (a
broker holdings snapshot) are taken as the demerger's shares when they cover
it; when they cover only part, that account is left unchanged and the output
says why. A tradebook import applies the demerger as it replays, and
re-importing it leaves both holdings as they were. Until it is applied,
`portfolio` (online or offline), `analyze`, `checkup`, `income`, `xray` and
`wrap` value a copy with the demerger in it and print "demerger pending".

**Imports.** A holdings export (Kite, INDmoney) has no purchase dates: existing
lots are kept and only the quantity is matched — more shares add a lot dated
today, fewer shares come out of the oldest lots (not recorded as a sale).
`trade holdings` syncs the same way. When the snapshot has more shares and a
split or bonus isn't applied to your lots yet, that holding is skipped until
you run `portfolio actions --apply` (otherwise the bonus shares would be
counted twice). A file with a date column (`Date`, `Purchase Date`,
`Buy Date`, `trade_date`, ...) is a list of lots: each symbol's lots are
rebuilt from it. With a buy/sell column (`trade_type`, `Type`, `Side`) it is a
tradebook — such as Zerodha Console's — sells come out FIFO as realized sales,
and splits and bonuses are applied as they happened (this needs a network
connection; Indian bonus issues become zero-cost lots, as in `portfolio
actions`). StockWise keeps the trades of each tradebook import and merges the
next file with them, so several yearly tradebooks (Console allows 365 days per
download) can be imported one after another, in any order, and re-importing a
file changes nothing. A symbol is left unchanged when its sells have no earlier
buy yet (the trades are kept; import the earlier tradebook to finish), or when
it already has lots or sales from elsewhere (manual entries, another import,
or a tradebook imported before StockWise kept trades) — `--replace` rebuilds it
from this file alone anyway. A file of dated buys only (no buy/sell column) is
the symbol's whole list of lots and replaces them. Dates may be
`YYYY-MM-DD`, `DD-MM-YYYY`, `DD/MM/YYYY` (`MM/DD/YYYY` with `-m us`) or
`15-Mar-2024`; a file with an unrecognized date is refused.

**Mergers.** A dated row (or `portfolio add --date`) for a ticker that was
merged away is restated in the acquirer's shares, keeping the date and the
total cost: HDFC up to the 13 Jul 2023 record date becomes HDFCBANK at 42 shares
for every 25, and Mindtree up to 24 Nov 2022 becomes LTIM at 73 for every 100
(quantity × ratio, price ÷ ratio). Rows without a date, and plain renames such
as ZOMATO → ETERNAL, are mapped to the new name only. Fractional shares from
the swap are kept as a fraction (the cash paid for them is not recorded).
Existing lots already imported under the new name are not corrected — re-import
the tradebook with `--replace`.

**Accounts.** Each lot and sale carries the demat account it came from, named
after the broker: `import kite` and `trade … --broker zerodha` are `zerodha`,
`import groww` is `groww`, and so on. A holdings file or tradebook only
matches, rebuilds or sells the lots of its own account, so the same stock held
at two brokers is two sets of lots in one holding (`portfolio lots` shows the
account). Lots entered by hand or saved by an older version, with no account of
their own, count as the account of their `source`, or — for manual lots — join
the first broker that syncs the symbol. Totals and tax span all accounts.
`portfolio sell` works on the whole holding, oldest lot first (or the lots
`--lots` names, for US and UK shares).

**Groww, Upstox, CAS.** Groww and Upstox exports are snapshots (quantity
sync, like Kite). A CAS whose transactions cover a fund's whole balance
rebuilds it as dated lots, with redemptions as realized sales; otherwise only
the quantity is synced. See [brokers.md](brokers.md#importing-holdings).

**Upgrading.** Portfolios saved before lot tracking are upgraded on first
load: each holding becomes one lot dated when it was added, and the original
file is kept once as `portfolio.json.bak`.

## Planning

| Command | Description |
|---------|-------------|
| `goal sip --target AMOUNT (--years N \| --months N \| --horizon 18mo \| --by YYYY-MM[-DD]) [--return PCT] [--inflation PCT] [--tax equity\|arbitrage\|debt\|none] [--slab PCT] [--exit-load PCT] [--net] [--glide-path] [--step-up PCT] [--savings AMOUNT \| --from-portfolio \| --holdings SYM,...] [--json]` | Monthly SIP that reaches the target. Without `--return` the return follows the horizon: capital preservation up to 24 months (6.75% India / 4.25% US, or the cached 364-day T-bill yield), conservative hybrid to 60 months (9% / 6.5%), long-term growth beyond (12% / 9%). Shows tax and exit load at redemption per instalment (equity: 20% STCG under 12 months, 12.5% LTCG above with the ₹1.25 L exemption; debt: slab rate; cess 4%), the net in hand and the SIP that leaves the target after tax (`--net` makes the target itself post-tax), P10 / median / P90 outcome bands with the chance of reaching the target, a ±2% return sensitivity, and a month-by-month (up to 24 months), quarterly (to 36) or yearly schedule. Forcing an equity return on a short goal prints a warning with the simulated downside |
| `goal retire --monthly-expense AMOUNT --years-to-retire N --years-in-retirement N [--inflation 6] [--return-pre 12] [--return-post 7] [--monthly-income AMOUNT] [--buffer-years N] [--legacy AMOUNT] [--step-up PCT] [--savings AMOUNT] [--json]` | Corpus needed on the day you retire to pay today's expenses (grown by inflation, less any pension or rent) for every month of retirement, and the SIP that builds it; the initial withdrawal rate, how long the corpus lasts after a 30% fall on day one, and the year-1 tax of an SWP against FD interest |
| `goal education --cost AMOUNT (--years N \| --months N \| --horizon D \| --by DATE) [--inflation 8] [--return PCT] [tax flags] [--json]` | Today's cost after the horizon of education inflation, and the SIP that pays it, with the same return basis, tax, bands and sensitivity as `goal sip` |
| `goal swp (--corpus AMOUNT \| --from-portfolio \| --holdings SYM,...) --withdrawal AMOUNT [--inflation 6] [--return 10] [--years 30] [--fd-return 7] [--slab 30] [--cost AMOUNT] [--json]` | How long the corpus lasts under monthly withdrawals that rise with inflation: an equity SWP (tax on the gain share only) against an FD (interest at the slab rate) |
| `goal emergency --monthly-expense AMOUNT [--months-cover 6] [--horizon 12mo] [--savings AMOUNT \| --from-portfolio \| --holdings SYM,...] [--return PCT] [--json]` | Reserve to hold, how much of it your liquid money covers, and the monthly build-up |
| `goal prepay --principal AMOUNT --loan-rate PCT --remaining-months N (--extra AMOUNT \| --lump-sum AMOUNT) [--return PCT] [--tax KIND] [--slab PCT] [--json]` | Exact interest and tenure saved by prepaying, against the after-tax SIP outcome of the same money (P10 / median / P90), the chance the SIP wins and the before-tax return it needs to break even |
| `goal add NAME --target AMOUNT --by YYYY-MM[-DD] [--inflation PCT] [--holdings SYM,...] [--return PCT]` | Save a goal in `goals.json` (a `--inflation` target is in today's money) |
| `goal [list] [--json]` · `goal show NAME [--json]` · `goal remove NAME` | Saved goals with months left, the live value of their linked holdings, the funded share and the SIP still needed; one goal in full; remove one |

`--from-portfolio` and `--holdings` value the portfolio (or the named holdings, such as `MF:122639` or `NIFTYBEES`) at live prices, or at cost when offline, and use that as existing savings; they replace `--savings`. Taxes and exit loads are estimates, not advice; `--tax` defaults to debt up to 60 months and equity beyond, with a 1% exit load on equity lots held under 12 months in India.

```bash
stockwise goal sip --target 1cr --years 15                  # ₹19,819 a month at 12%
stockwise goal sip --target 1cr --years 15 --step-up 10     # ₹11,516 a month, rising 10% a year
stockwise goal sip --target 10L --months 9                  # short goal: 6.75% baseline, month-by-month table
stockwise goal sip --target 50L --by 2031-06 --inflation 6 --from-portfolio
stockwise goal retire --monthly-expense 80k --years-to-retire 20 --years-in-retirement 30
stockwise goal education --cost 25L --years 10 --savings 3L
stockwise goal add house --target 50L --by 2031-06 --holdings NIFTYBEES
stockwise goal swp --corpus 1cr --withdrawal 50k
stockwise goal prepay --principal 50L --loan-rate 9 --remaining-months 240 --extra 10k
```

Amounts take Indian units — `50k`, `25L` / `25 lakh`, `1cr` / `1.5 crore`,
`2,50,000` — and results print in lakhs and crores (dollars with `-m us`).
The math, as Indian SIP calculators do it: the monthly rate is the annual
return ÷ 12 and SIPs are paid at the start of each month; a step-up raises
the SIP once a year; `--savings` already invested grows at the same return
and lowers the SIP. In retirement the expense rises with inflation once a
year and the corpus earns `--return-post` until it runs out at the end of the
last year. Returns are assumptions, not promises.

## AI assistants

> **Guide:** [Use StockWise from Claude, Gemini, Codex or Cursor](guide-mcp.md).

Both use the same tools (quotes, history, technicals, risk, scorecards,
insights, events, mutual funds, option chains, IPOs, screens, picks, market
conditions, backtests, your portfolio dashboard, lots, tax estimate, advance
tax, harvest and rebalance plans, calendar, goals, watchlist, alerts,
`fit_check` against your own rules, and the rule-based [actions](#actions)
from `stock_action` and `portfolio_review`). They read, unless you turn on the
workspace tools; `undo` reverses what those change. The model explains the
action the rules computed; it never invents or changes one, and never makes up
a price level. See [ai.md](ai.md).

| Command | Description |
|---------|-------------|
| `ask [QUESTION] [-i, --interactive] [-c, --continue \| --resume ID] [--sessions] [--playbook checkup\|tax-season\|earnings] [--auto] [--verbose] [--max-steps 6] [--max-tokens 100000] [--json]` | Ask in plain English; the configured model (local Ollama by default, or the cloud provider in `STOCKWISE_AI_PROVIDER`) calls the tools and answers, knowing your investor profile. Needs a model with tool calling. `-i` keeps asking until `exit`; `-c` carries on the newest saved conversation, `--resume ID` a given one, `--sessions` lists them (the newest 50, in `cache.db`). `--playbook` runs a prepared task. `--auto` lets the model change your watchlist, alerts, goals, targets, notes and profile (undo with `undo`). `--verbose` prints each tool call on stderr; `--json` prints the transcript (answer, tool calls with arguments, token usage, conversation id). The last request runs without tools, so the model must answer. Educational, not financial advice |
| `mcp [--http ADDR] [--allow-remote] [--allow-orders] [--allow-writes] [--allow-portfolio] [--list-tools]` | [Model Context Protocol](https://modelcontextprotocol.io) server for Claude, Codex, Gemini, Antigravity, Cursor, VS Code and other assistants: 71 tools that read, seven resources (portfolio, watchlist, alerts, profile, latest picks, track record, methodology) plus a quote template, and twelve prompts. Stdio by default; `--http 127.0.0.1:8788` serves Streamable HTTP at `/mcp` (other addresses need `--allow-remote` and `STOCKWISE_API_TOKEN`). `--allow-writes` adds 11 workspace tools (watchlist, alerts, goals, targets, notes, profile and profile onboarding); `--allow-portfolio` also adds `portfolio_add` and `portfolio_sell`, which record lots. Each change is journalled for `undo`; neither works with `--allow-remote`. `--allow-orders` adds a dry-run `order_preview` that returns the `stockwise trade` command for you to run — it never places orders. `--list-tools` prints the tools and their input schemas as JSON. Cargo feature `mcp` (on by default) |
| `connect [APP...] [--all] [--list] [--remove] [--allow-writes] [--allow-portfolio] [--no-skill] [--dry-run] [--json]` | Add the MCP server to AI apps: Claude Desktop and Code, Cursor, Windsurf, VS Code, Cline, Gemini CLI, Antigravity, Codex, Zed, LM Studio and opencode (Goose and Continue get lines to paste). With no app named, every app found on this computer. Only the `stockwise` entry changes and each config is backed up to `<file>.stockwise.bak`; the agent skill is installed for Claude Code and Antigravity. `--remove` takes it out again |
| `undo [ID] [--list] [--force] [--json]` | Reverse a change an assistant made (`mcp --allow-writes`, `ask --auto`): the newest by default, or change `ID`. `--list` shows the journal. It refuses when a later change or your own edit touched the same file; `--force` undoes anyway |

```bash
stockwise ask "how is my portfolio doing?"
stockwise ask -i
stockwise ask --playbook checkup
stockwise connect                             # every AI app on this computer
stockwise connect antigravity --allow-writes
stockwise undo --list
stockwise mcp --list-tools | jq '.[].name'
```

## Interfaces

Both are read-only and share the data layer with the commands above, so they
show the same numbers. They are on by default and can be left out of a build
with `--no-default-features` (cargo features `tui` and `server`).

| Command | Description |
|---------|-------------|
| `tui [-i SECONDS]` | Full-screen live dashboard: indices, watchlist, portfolio P&L, alerts and a chart of the selected symbol. Refreshes every 30s by default (10-3600). `?` shows the keys. See [tui.md](tui.md) |
| `serve [--bind ADDR] [--allow-remote] [--open]` | Web dashboard at `/` (`--open` opens it in your browser), JSON API (`/api/quote`, `/api/history/{symbol}`, `/api/markets`, `/api/search`, `/api/portfolio`, `/api/watchlist`, `/api/alerts`), `/healthz` and Prometheus `/metrics`. Listens on `127.0.0.1:8787`; other addresses need `--allow-remote` and `STOCKWISE_API_TOKEN`. See [server.md](server.md) |
| `statusbar [--format tmux\|waybar\|sketchybar\|starship\|json] [--symbols A,B] [--max N] [--refresh SECS]` | One compact line for a status bar: the portfolio's day change, then its largest holdings and watchlist symbols (or `--symbols`). Answers from a local cache and fetches at most once a minute. See [Status bars](#status-bars) |

### Status bars

`stockwise statusbar` prints one line such as
`▲ ₹+12.3k (+0.8%) · RELIANCE +1.2% · TCS −0.4%`: the portfolio's day change
in its base currency (the same one `dashboard` totals in), then its largest
holdings by value, then watchlist symbols, up to `--max` (default 3).
`--symbols RELIANCE,TCS` shows those symbols instead of the portfolio.

Status bars run it every few seconds, so it is built to be quick and never
hang:

- It answers from `statusbar_cache.json` in the data directory. It goes to the
  network at most once per `--refresh` seconds (default 60, at least 5), and the
  whole refresh (quotes and exchange rates) must finish within 3 seconds.
- When a refresh fails or times out, the cached line is shown with
  `(as of HH:MM)` and the waybar class `stale`. A failed attempt also waits out
  `--refresh` before the next one, so a dead network costs one try a minute.
- With every market of the shown symbols closed, it fetches once after the
  close (for the first 15 minutes, while closing prices settle) and then stays
  offline until the next open.
- It always exits 0. With no holdings it says so; offline with no cache it
  prints `stockwise: offline (no data)`.

| `--format` | Output |
|---|---|
| `tmux` (default) | `#[fg=green]…#[default]` / `#[fg=red]` colour codes |
| `waybar` | One JSON object `{"text","tooltip","class"}`; `class` is `up`, `down`, `flat` or `stale`, the tooltip lists the value, each price and the time. `&`, `<`, `>` are Pango-escaped |
| `sketchybar` | `label='…' label.color=0xAARRGGBB`, shell-quoted, to `eval` after `sketchybar --set "$NAME"` |
| `starship` | Plain text, no colour codes |
| `json` | `{"text","as_of","stale","market_open","class","error","portfolio":{"currency","value","day_change","day_change_pct"},"rows":[{"symbol","price","change_pct","currency"}]}`; `portfolio` is `null` with `--symbols` or no holdings |

**tmux** (`~/.tmux.conf`; tmux re-runs it every `status-interval` seconds):

```tmux
set -g status-interval 10
set -g status-right '#(stockwise statusbar) '
set -g status-right-length 80
```

**waybar** (`~/.config/waybar/config`, then add `"custom/stockwise"` to a
`modules-*` list; style `#custom-stockwise.up`, `.down`, `.stale` in
`style.css`):

```json
"custom/stockwise": {
    "exec": "stockwise statusbar --format waybar",
    "return-type": "json",
    "interval": 15
}
```

**sketchybar** (`~/.config/sketchybar/sketchybarrc`):

```sh
sketchybar --add item stockwise right \
           --set stockwise update_freq=15 script="$PLUGIN_DIR/stockwise.sh"
```

and `$PLUGIN_DIR/stockwise.sh` (executable):

```sh
#!/bin/sh
eval "sketchybar --set \"\$NAME\" $(stockwise statusbar --format sketchybar)"
```

**starship** (`~/.config/starship.toml`):

```toml
[custom.stockwise]
command = "stockwise statusbar --format starship"
when = true
shell = ["sh"]
format = "[$output]($style) "
style = "dimmed"
```

The prompt waits for the command, but a cached line returns in milliseconds; a
refresh is bounded by the 3 second budget.

## Live trading

See [brokers.md](brokers.md). Orders are previewed and need a typed `yes`.
Every `trade` command takes `--broker angel|zerodha|upstox|dhan|fyers`
(default `angel`, or `STOCKWISE_BROKER`); all but `angel` are experimental —
untested against a live account.

| Command | Description |
|---------|-------------|
| `trade setup` / `trade config [--keyring]` | How to connect / create the owner-only credentials file, move secrets into the OS keyring |
| `trade login` | Daily login (Zerodha, Upstox, Fyers) or paste a Dhan token |
| `trade holdings` / `trade positions` / `trade orders` | Live holdings (synced into `portfolio`, keeping your lots), open positions, order book |
| `trade buy <SYMBOL> <QTY> [--limit P] [--trigger T] [--dry-run] [--yes]` | Buy on NSE: MARKET, LIMIT, SL-M (`--trigger`) or SL (both) |
| `trade sell <SYMBOL> <QTY> [--limit P] [--trigger T] [--dry-run] [--yes]` | Sell on NSE, same order types |
| `trade limit <SYMBOL> <QTY> <PRICE> [--dry-run] [--yes]` | Limit buy on NSE |
| `trade gtt buy` / `trade gtt sell` `<SYMBOL> <QTY> --trigger T --limit P [--dry-run] [--yes]` | Single-leg GTT (not Upstox) |
| `trade gtt list [--notify] [--json]` / `trade gtt cancel <ID>` | List GTTs with status changes since the last look (triggered, cancelled, expired, gone; `--notify` sends them) / cancel one (asks for confirmation) |
| `trade sync [--since DATE] [--apply] [--notify] [--json]` | Today's executed delivery trades into the portfolio — buys become dated lots, sells come out FIFO — idempotent by broker trade id; previews unless `--apply`. Dhan also reads older fills with `--since`. See [brokers.md](brokers.md#syncing-trades-and-gtts-into-the-portfolio) |
| `trade selftest [--redact-amounts] [--json]` | Read-only check of the integration against your account (session, profile, holdings, positions, orders, trades, GTTs, instrument lookup, a dry-run order, response shapes); prints a redacted report for a [broker test report](brokers.md#help-verify-a-broker). Exits 1 if a check failed |

## Setup

| Command | Description |
|---------|-------------|
| `profile` / `profile show [--json]` | Show active investor profile mode (`passive`, `active:low\|med\|high`, `ultra`), effective tax parameters and personal guardrails |
| `profile set [MODE[:RISK]] [--risk low\|med\|high] [--tax-slab PCT] [--tax-regime new\|old] [--horizon-years N] [--max-stock-weight PCT] [--max-smallcap-weight PCT] [--risk-per-trade PCT] [--residence in\|us\|uk] [--json]` | Save investor profile mode, personal guardrails and tax residence (holding periods and tax rules follow it) to `config.toml` |
| `profile reset` | Reset profile mode and guardrails in `config.toml` to defaults (`active:med`) |
| `config [show] [--json]` | Effective settings and where each comes from (flag, env, config file, default), plus the config file's status |
| `config path` | Print the config file path (it may not exist yet) |
| `config init [--force]` | Write a commented template to the config file path; refuses to replace an existing file without `--force` |
| `backup export [PATH.swbak] [--passphrase-fd N]` | Write your data to one passphrase-encrypted file (default `stockwise-YYYY-MM-DD.swbak`). See [Backups](#backups) |
| `backup restore <PATH.swbak> [--dry-run] [--passphrase-fd N]` | Check a backup, list the files it would add or replace, copy each replaced file to `<file>.bak`, then write each file atomically |
| `notify` / `notify status` / `notify test` | Show the configured notification channels (Telegram, Slack, Discord, email, ntfy, Pushover, desktop), their routes and quiet hours / send a test message. See [notifications](bots.md#notifications) |
| `monitor` / `monitor status [--json]` / `monitor run [--notify] [--json]` | The automatic holding monitor: thresholds, worker state, recent signals; `run` does one pass now (`--notify` sends what is new). See [Holding monitor and digest](#holding-monitor-and-digest) |
| `digest [--send] [--json]` | Today's after-close digest: day P&L, top contributors and detractors with reasons, monitor signals, triggered alerts, notices, dates coming up. `--send` sends it now |
| `telegram [--once]` | Two-way Telegram bot: answers `/today`, `/hot`, `/review` and `/digest` from `STOCKWISE_TELEGRAM_CHAT_ID` only, and sends the digest every trading day at `STOCKWISE_DIGEST_TIME` (local `HH:MM`) unless the worker already has |
| `update [--check]` (`self-update`) | Check GitHub Releases for a newer version. A binary from install.sh or a release archive is replaced in place: the archive for this platform and its `.sha256` are downloaded, the checksum must match (a missing or wrong checksum stops the update and changes nothing), then the binary is swapped atomically (on Windows the old one is kept as `stockwise.exe.old`). Installs from Homebrew, Scoop, winget, Nix, Cargo, npm, pipx, uv, pip, `.deb`/`.rpm` or Docker are never changed; the command prints that package manager's upgrade command instead. `--check` only reports the installed version, the latest release and the install channel |
| `completions <bash\|zsh\|fish\|powershell\|elvish>` | Print a completion script, e.g. `stockwise completions zsh > ~/.zfunc/_stockwise` |
| `man [--dir <DIR>]` | Print the `stockwise.1` man page, or write it plus one `stockwise-<command>.1` page per command into `DIR`, e.g. `stockwise man --dir ~/.local/share/man/man1` |

### Backups

`backup export` packs your own data into one encrypted file you can keep in
Git, iCloud, Dropbox, Google Drive or S3, or carry to another computer;
`backup restore` puts it back.

```bash
stockwise backup export ~/Dropbox/stockwise.swbak
stockwise backup restore ~/Dropbox/stockwise.swbak --dry-run   # list changes only
stockwise backup restore ~/Dropbox/stockwise.swbak
STOCKWISE_BACKUP_PASSPHRASE="$(cat ~/.config/stockwise/backup-pass)" \
  stockwise backup export nightly.swbak                                   # cron or CI
stockwise backup export nightly.swbak --passphrase-fd 3 3< ~/.config/stockwise/backup-pass
```

**What is in it**: from the data directory (`STOCKWISE_HOME`),
`portfolio.json`, every named portfolio (`portfolios/*.json`) and wealth
history, `goals.json`, `watchlist.json`, `alerts.json`, `notes.json`, the
undo log (`audit.json`) and the practice files (`sim_*.json`), plus
`config.toml`. **Left out**: caches (`cache.db*`, `cache/`, `*_cache.json`),
lock and temp files, rotating backups (`portfolio.json.1`..`.5`, `*.bak`),
daemon and worker state, and all broker files (`*_config.json`,
`gtt_seen_*.json`), because they hold session tokens. Notification secrets
(`telegram_token`, `smtp_password`, ...) are removed from the copy of
`config.toml`, so a backup never holds them. Keep them on the source computer
(or in a password manager). After a restore that replaces `config.toml`, the
restore names the `config.toml.bak*` copies that still hold secrets (those made
from a config from before any restore). On a new computer there is no such
copy, so take them from the source computer's `config.toml` or set them as
`STOCKWISE_*` environment variables. A secret
written in a form the backup cannot remove (an inline `notify = { ... }` table,
or a value that spans lines) stops the export with a message instead of being
copied.

**Passphrase**: by default StockWise asks for it with a hidden prompt (twice
on export; at least 8 characters). For scripts, pass `--passphrase-fd N` to
read the first line of an open file descriptor (Unix), or set
`STOCKWISE_BACKUP_PASSPHRASE`. There is no argument that takes the passphrase
itself, so it never shows up in the process argument list. Set the variable
from a protected file or a secret store, not by typing the passphrase in an
interactive shell, where it is saved in the shell history; other processes of
the same user can also read a process's environment. A lost passphrase cannot
be recovered.

**Restore** first checks the whole file: a wrong passphrase, a changed byte
or a truncated file stops it with an error before anything is written. It then
lists each file as `add`, `replace` or `same`; `--dry-run` stops there.
Otherwise it locks every file, reads it again (it may have changed since the
list was made), and copies each file that differs from the backup to
`<file>.bak` (or `<file>.bak.1`, `.bak.2`, ... when an earlier copy exists, so
no copy is ever overwritten). Then every file is written to a temp file and
renamed into place. The restore is all or nothing: if one write fails, the
files already written get their previous content back and files it added are
removed, so the data directory ends as it started. Local files that are not in
the backup are left alone. A replaced file keeps its permissions (a `chmod 600`
`config.toml` stays owner-only); new files and the `.swbak` file itself are
owner-only. Both commands print the data directory they use. When the config
file is invalid, its `home` setting cannot be read, so `backup` refuses to run
unless `STOCKWISE_HOME` names the data directory.

**Format** (`.swbak`, version 1): a 64-byte header (the magic `SWBAK\0\r\n`,
format version, algorithm ids, Argon2id memory/time/parallelism costs, a
random 16-byte salt and a random 24-byte nonce), then the archive encrypted
with XChaCha20-Poly1305 under a key derived from the passphrase with
Argon2id (64 MiB, 3 passes). The header is authenticated with the data. The
archive inside is deterministic: files sorted by path, no timestamps or
owners. The salt and nonce are new for every export, so two exports of the
same data are different files.

## Scripting with `--json` and `--csv`

> **Guide:** [Script it with JSON, CSV and jq](guide-scripting.md).

```bash
stockwise portfolio --json | jq '.totals'
stockwise portfolio --json | jq -r '.holdings | sort_by(.pnl_percent) | .[:3][] | "\(.symbol) \(.pnl_percent)%"'
stockwise portfolio --csv > portfolio.csv                 # open in a spreadsheet
stockwise history TCS -p 1y --csv > tcs.csv               # every daily bar
stockwise alert check --json | jq -r '.[] | select(.status == "triggered") | .symbol'
stockwise screen dividend --csv | column -s, -t
stockwise tax in --json | jq '.realized.total_tax'
stockwise tax export in --fy 2025-26 --json | jq '.summary.taxable_ltcg'
stockwise portfolio benchmark --json | jq '{portfolio_return, benchmark_return, beta: .metrics.beta}'
stockwise goal sip --target 1cr --years 15 --json | jq '.monthly_sip'
for s in rsi macd sma bb vwap mr; do
  stockwise backtest INFY -s "$s" --json | jq -r '"\(.strategy)\t\(.total_return_percent)"'
done
for s in $(stockwise watch --json | jq -r '.[].symbol'); do
  stockwise technical "$s" --json | jq -r '"\(.symbol)\t\(.rsi_14)\t\(.bias.verdict)"'
done
stockwise quote TCS INFY --json | jq '.[] | {symbol, regularMarketPrice}'
stockwise markets --json | jq '.[] | select(.change_percent < -1)'
stockwise mf sip 122639 5000 -p 3y --json | jq '{invested, value, xirr}'
stockwise options chain NIFTY -n 2 --json | jq '.rows[] | {strike, delta: .call.greeks.delta}'
stockwise options payoff NIFTY -s straddle --json | jq '{breakevens, pop, max_loss}'
stockwise mf screen flexi cap -s ter --json | jq '.[] | {name, ter, return_3y}'
```

**Rules**

- stdout holds only the JSON or CSV; errors go to stderr with exit code 1, so
  `set -e` and `||` work. `--json` and `--csv` can't be combined.
- Empty states are not errors: an empty portfolio prints `{"holdings": [], ...}`
  or a CSV header with no rows.
- Keys are stable `snake_case`. The exceptions are `quote --json` and
  `search --json`, which pass Yahoo's own fields through (camelCase, unchanged
  since 0.3); their `--csv` uses snake_case columns.
- A value that isn't available is `null` in JSON and an empty cell in CSV —
  never `0`. A holding without a live price keeps its row but is left out of
  totals and listed in `unpriced`.
- Fields ending in `_percent` are percentages (`12.5` = 12.5%). Older keys
  that end in `_pct` (as in the shapes below) are percentages too, and every
  one also appears as its `_percent` twin right after it (`weight_pct` and
  `weight_percent`); use the `_percent` name in new scripts, as the `_pct`
  names go in 1.0. `dividend_yield`
  in `quote`, `compare` and `screen` is a fraction as Yahoo reports it (`0.012`
  = 1.2%). Prices are as Yahoo returns them; derived money values are rounded
  to 2 decimals, indicators to 4.
- Dates are `YYYY-MM-DD`; `history` bar times are RFC 3339 in UTC.
- CSV follows RFC 4180 quoting (fields with a comma, quote or newline are
  quoted, quotes doubled), starts with a header row, and ends lines with `\n`.
  CSV columns are the same names as the JSON row keys.
- The `--ai` section is never part of JSON or CSV output.
- Every `--json` output that has a stock call carries an `action` object
  (`verdict`, `deep`, `portfolio`, `portfolio checkup`, `portfolio review`,
  `picks`): `{kind, label, confidence, reasons, invalidation_price, review_by,
  size_hint: {max_weight_pct, shares, limited_by}, tax_note, horizon, held,
  coverage, missing, currency, methodology, disclaimer}`. `kind` is `BUY`,
  `WATCH`, `AVOID`, `ADD`, `HOLD`, `TRIM`, `EXIT` or `NO_CALL`.
  `size_hint.shares` and `missing` are left out when empty; other missing
  values are `null`.
- `mf` and `options` have their own `--json` (no `--csv`); fractions there are
  decimals (`0.1234` = 12.34%), and missing values are `null`, never 0.
- `options payoff --json` is `{symbol, source, underlying, currency, as_of, expiry,
  days_to_expiry, strategy, width, lot_size, lot_size_from, lots, multiplier, rate,
  div_yield, legs: [{side, qty, instrument, strike, price, price_from, iv, iv_from,
  delta}], net_premium, net_premium_total, premium_type, max_profit, max_loss,
  breakevens, profit_ranges: [{from, to}], pop, pop_iv, pop_iv_from, greeks: {delta,
  gamma, theta, vega, rho}, notes, curve: [{price, expiry, today}]}`. `net_premium`
  and leg prices are per unit (positive = debit); money totals, greeks and the
  curve are for the whole position (`× multiplier` = lot size × lots).
  `max_profit`/`max_loss` are `null` when unlimited; `max_loss` is negative;
  `to: null` means "and above". `price_from` is `mid`, `ltp`, `manual` or `spot`.
  Money values, prices and breakevens are rounded to 2 decimals.

**Shapes** (`[...]` is an array; `row` is described next to it)

| Command | `--json` | `--csv` — one row per |
|---------|----------|-----------------------|
| `quote` | `[quote]` — Yahoo's quote fields, plus `cap_tier` and `dataSource` when another provider supplied the price, and `deliveryPercent`, `lowerCircuit`, `upperCircuit` when NSE did ([data-providers.md](data-providers.md#nse-and-bse)) | quote: `symbol, name, currency, price, change, change_percent, open, day_low, day_high, year_low, year_high, volume, avg_volume_3m, market_cap, cap_tier, pe_ttm, eps_ttm, dividend_yield, delivery_percent, lower_circuit, upper_circuit` (the last three empty unless NSE answered) |
| `markets` | `[{region, symbol, name, price, change, change_percent, currency}]` — `region` is `Indian Markets`, `US Markets`, `Crypto`, `FX`, `Commodities`, a country (`United Kingdom`, `Germany`, `Japan`, `Hong Kong`, `Singapore`, `Canada`, `Australia`) or `Global`. The India 10-year yield is the row with `symbol` `IN10Y`: `price` is the yield in percent, `currency` and `change` are null | index |
| `search` | `[hit]` — Yahoo's search fields | hit: `symbol, name, quote_type, exchange` |
| `commodity` / `commodity spot` | `[{metal, unit, price, currency, change_percent, source}]` — `metal` is `gold` or `silver`; `unit` is `USD/oz`, `INR/g`, `INR/10g` or `INR/kg` | row |
| `commodity futures`, `commodity etfs` | `[{symbol, metal, name, contract, price, change, change_percent, volume, currency}]` — `contract` is Yahoo's short name (the front month for futures) | instrument |
| `commodity sgb` | `[{symbol, isin, series, issue_date, issue_price, maturity, early_redemption_from, years_to_maturity, price, change_percent, volume, gold_value, premium_percent, interest_per_year, current_yield_percent}]` — `price` is ₹ per unit (1 g) and null when NSE gave none; `gold_value` is 1 g at the international spot price; `maturity` is nominal (issue + 8 years); NSE's notes go to stderr | tranche |
| `bonds` / `bonds yields` | `[{date, tenor, kind, security, ytm, source}]` — `kind` is `tbill`, `gsec` or `sdl`; `source` is `CCIL` or `RBI`; `ytm` in percent, null when the page shows none | yield |
| `bonds list` | `[{isin, name, coupon, issue_date, maturity, outstanding_cr}]` — `coupon` is null for floating-rate bonds; `outstanding_cr` in ₹ crore | security |
| `bonds calc` | `{security, isin, kind, coupon, maturity, settlement, days_to_maturity, clean_price, accrued_interest, dirty_price, ytm, current_yield, macaulay_duration, modified_duration, next_coupon, coupons_left}` — `kind` is `gsec` or `tbill`; prices per ₹100 face value, yields in percent | — |
| `history` | `{symbol, currency, period, interval, bars: [{time, open, high, low, close, volume}]}` | bar |
| `technical` | `{symbol, currency, period, interval, bars, price, sma_10, sma_20, sma_50, sma_100, sma_200, ema_12, ema_26, rsi_14, rsi_signal, macd: {line, signal, histogram, trend}, bollinger: {upper, middle, lower, position}, atr_14, atr_percent, vwap, bias: {verdict, bullish, bearish}}` — `position` is `above_upper`, `below_lower` or `within`; `verdict` is `bullish`, `bearish` or `neutral` | — |
| `risk` | `{symbol, currency, period, interval, bars, price, return_percent, volatility_percent, risk_level, sharpe, sortino, calmar, max_drawdown: {percent, peak_price, trough_price, trading_days}, var_95_percent, var_99_percent}` — `risk_level` is `low`, `moderate`, `high` or `very_high` | — |
| `compare` | `[{symbol, name, currency, price, change_percent, market_cap, cap_tier, pe_ttm, pe_forward, eps_ttm, price_to_book, dividend_yield, profit_margin, revenue_growth, debt_to_equity, beta, year_high, year_low, sma_50, analyst_mean_rating}]` — the table's transpose | stock |
| `correlate` | `{period, interval, symbols: [..], matrix: [[..]]}` — `matrix[i][j]` pairs `symbols[i]` and `symbols[j]` | symbol: `symbol, <SYM1>, <SYM2>, ...` |
| `returns` | `{symbol, currency, price, returns: [{period, label, start_price, end_price, return_percent, annualized_percent, growth_of_100000}]}` — `annualized_percent` is `null` for spans under a year | period |
| `movers` | `{gainers: [row], losers: [row]}`, row = `{side, rank, symbol, price, change_percent, volume, currency, cap_tier}` | stock, gainers then losers |
| `sectors` | `[{sector, symbol, price, change_percent, currency}]`, best first | sector |
| `screen` | `[{symbol, name, price, change_percent, pe, market_cap, dividend_yield, currency, cap_tier}]` | match |
| `dividends` | `{symbol, currency, price, pays_dividend, dividend_yield_percent, annual_dividend_per_share, income: [{invested, shares, annual_income, monthly_income}], drip: [{year, shares, growth_percent}]}` | — |
| `backtest` | `{symbol, currency, strategy, period, interval, bars, start, end, fill, capital, start_value, end_value, total_return_percent, cagr_percent, max_drawdown_percent, sharpe, sortino, exposure_percent, trade_count, win_rate_percent, avg_win_percent, avg_loss_percent, profit_factor, costs_paid, gross_return_percent, buy_hold_return_percent, alpha_percent, skipped_entries, costs, cost_model, rules, trades: [trade], equity: [{date, value}]}` — returns are after costs except `gross_return_percent`; `portfolio` and `walkforward` shapes are in [backtesting.md](backtesting.md#json-output). trade = `{symbol, entry_date, exit_date, qty, entry_price, exit_price, gross_pnl, costs, net_pnl, pnl_percent, bars_held, exit_reason}` | trade (every form) |
| `sip` | `{symbol, currency, period, interval, monthly_amount, months, total_invested, final_value, profit, total_return_percent, xirr_percent, total_units, avg_cost, current_price}` | — |
| `whatif` | `{symbol, currency, amount, date, buy_date, buy_price, shares, invested, current_price, value, pnl, pnl_percent, cagr_percent, days, years}` — `buy_date` is the trading day closest to `date` | — |
| `portfolio` | `{holdings: [holding], totals: {cost_basis, value, pnl, pnl_percent, currency}, unpriced: [symbol], unconverted: [symbol], notes: [text]}`, holding = `{symbol, shares, avg_cost, cost_basis, price, value, pnl, pnl_percent, weight_percent, xirr_percent, currency, name, price_source, nav_date}` — a holding's money values are in its own currency; `totals` and `weight_percent` are in the base currency (`--base`); `unconverted` holdings had no exchange rate; `price_source` is `quote`, `amfi_nav`, `nse_sgb`, `gold_estimate` or null (no price) and `nav_date` the date of a fund's NAV; `notes` explain fund and SGB prices (unknown codes, estimates) | holding |
| `watch` | `[{symbol, price, change, change_percent, volume, currency}]`, in watchlist order | symbol |
| `alert list` | `[{index, symbol, condition, target, created_at, period, description}]` — `index` is what `alert remove` takes; `condition` is the CLI name (`above`, `rsi-below`, ...), `period` the RSI or volume-average length (else null), `description` the condition in English words | alert |
| `alert check` | `[{index, symbol, condition, target, price, currency, status, period, description, value, reference}]` — `status` is `triggered`, `waiting`, `no_price` (price alert without a live price) or `no_data` (too little history, or no results date); `value` is what the condition compared (price, RSI, day change %, volume multiple, days to results; the close for crosses and breakouts) and `reference` the moving average, previous high/low or average volume | alert |
| `calendar` | `[{date, days_until, when, symbol, event, detail, source, estimate}]` — `when` is `today`, `tomorrow`, `this_week` (within 7 days) or `later`; `event` is `results`, `board_meeting`, `ex_dividend`, `dividend_paid`, `split`, `bonus`, `rights` or `other`; `source` is `yahoo` or `nse`; `estimate` marks Yahoo's unconfirmed earnings dates. Notes (e.g. NSE refused) go to stderr | event |
| `portfolio benchmark` | `{benchmark: {symbol, name, currency, converted_to_base}, base_currency, period, start, end, trading_days, portfolio_return, benchmark_return, excess_return, portfolio_annualized, benchmark_annualized, metrics: {observations, alpha, beta, tracking_error, information_ratio, correlation}, holdings: [holding], sectors: [{sector, holdings, avg_weight, end_weight, return_pct, contribution, active_contribution}], flagged: [{symbol, reason}], series: [{date, portfolio, benchmark}]}`, holding = `{symbol, sector, avg_weight, end_weight, return_pct, contribution, active_contribution, days_held, days_unpriced, flag}` — returns, weights, `alpha` and `tracking_error` are percentages, contributions are percentage points; annualized returns only for periods over about 13 months; `series` is growth of 100; figures are null when nothing was held | holding |
| `portfolio pme` | `{summary: {benchmark_symbol, benchmark_name, base_currency, start_date, end_date, total_invested, total_distributed, actual_value, twin_value, value_added, kaplan_schoar_pme, direct_alpha_percent, portfolio_xirr_percent, twin_xirr_percent, twin_shares, benchmark_end_price}, holdings: [{symbol, actual_value, twin_value, value_added, pme_ratio, direct_alpha_percent, actual_xirr_percent, twin_xirr_percent, cashflows_count}]}` — Public Market Equivalent (Index Twin) analysis; `kaplan_schoar_pme` is the KS-PME ratio (> 1.0 indicates outperformance); `direct_alpha_percent` is the annualized excess return rate (%) | holding row |
| `portfolio xray` | `{portfolio_name, base_currency, total_value, effective_holdings: [{symbol, name, direct_value, direct_weight_pct, fund_value, fund_weight_pct, total_value, effective_weight_pct, is_concentrated, via_funds: [{fund_symbol, value, weight_in_fund_pct, effective_weight_pct}]}], concentrations_above_10pct: [effective_holding], market_cap_breakdown: {large_cap_value, large_cap_pct, mid_cap_value, mid_cap_pct, small_cap_value, small_cap_pct, unclassified_value, unclassified_pct}, sector_allocation: [{sector, value, weight_pct}], asset_class_split: {equity_value, equity_pct, debt_value, debt_pct, commodity_value, commodity_pct, cash_value, cash_pct}, funds_analyzed: [{symbol, name, look_through_available, holdings_count, coverage_pct}]}` | effective holding |
| `tax export` | `{country, tax_year, period_start, period_end, currency, disclaimer, sales: [sale], summary, other_currency: [symbol], virtual_digital_assets: [symbol], intraday_sales, intraday_gain, other_rules: [symbol], notes: [text]}` (`virtual_digital_assets`, `intraday_sales`, `intraday_gain` and `other_rules` for India only) — India: sale = `{section, term, isin, scrip, symbol, quantity, sale_date, sale_price, sale_value, purchase_date, purchase_price, cost, actual_cost, fmv_31_jan_2018, holding_days, gain, grandfathered, purchase_date_estimated}`, summary = `{stcg_111a_sale_value, stcg_111a_cost, stcg_gains, stcg_losses, ltcg_112a_sale_value, ltcg_112a_cost, ltcg_gains, ltcg_losses, st_loss_set_off_against_ltcg, taxable_stcg, ltcg_after_set_off, ltcg_exemption, taxable_ltcg, taxable_stcg_before_23_jul_2024, taxable_ltcg_before_23_jul_2024, stcg_tax, ltcg_tax, cess, total_tax, st_loss_carried_forward, lt_loss_carried_forward}`; US: sale = `{part, description, symbol, quantity, date_acquired, date_sold, proceeds, cost_basis, code, adjustment, gain_or_loss, date_acquired_estimated, holding_days_added}` (`code` is `W` for a wash sale, `adjustment` the disallowed loss, `holding_days_added` the days carried over from washed shares), summary = `{short_term_proceeds, short_term_cost, short_term_adjustment, short_term_gain, long_term_proceeds, long_term_cost, long_term_adjustment, long_term_gain, net_gain, taxable_short_term, taxable_long_term, loss_deductible_this_year, loss_carryover, short_term_tax, long_term_tax, total_tax}`, and after `summary` the US adds `wash_sales: [{sale, part, symbol, sell_date, qty, loss, replacement_qty, disallowed, replacements: [{date, date_estimated, qty, basis_added, days_added, sale}]}]` (`sale` is the index in the portfolio's realized sales and `part` the row of that sale; a replacement's `sale` is null while it is still held) | — (the CSV is the schedule itself, see [Tax export](#tax-export)) |
| `harvest` | `{country, currency, total_st_loss_harvestable, total_lt_loss_harvestable, estimated_loss_tax_saved, ltcg_exemption_limit, ltcg_exemption_remaining, loss_candidates: [row], gain_harvest_candidates: [row], fifo_blocked: [{symbol, shares_to_sell, net_result, lot_loss}], unpriced: [symbol], other_rules: [{symbol, reason}], other_currency: [symbol], carried_forward: {short_term, long_term}, tax_free_gain_room, notes: [string]}`, row = `{symbol, shares, avg_cost, price, pnl, pnl_pct, st_pnl, lt_pnl, estimated_tax_saved, harvest_type, recent_buy, wash_sale_window_end, lots}` — `harvest_type` is `tax_loss` or `ltcg_exemption_step_up`; `recent_buy` and `wash_sale_window_end` are set for US loss candidates only (see [harvest](#harvest)); for India `shares`, `st_pnl` and `lt_pnl` describe the whole FIFO sale that reaches the losing lot; `fifo_blocked` holdings would book a net gain (`net_result` > 0) if sold up to the losing lot, so they are not offered as losses; `lots` is set when a US loss sells other lots than the oldest (record it with `portfolio sell --lots`); a step-up row's `shares` is the exact count whose gain fits `tax_free_gain_room`; `other_currency` and `other_rules` holdings are left out | row |
| `rebalance` | `{total_value, projected_value, currency, inflow, drift_tolerance, mode, raise, cash_buffer, sell_proceeds, buy_cost, costs, net_cash, cash_left, tax: {st_gain, lt_gain, tax, fifo_tax, saved_vs_fifo, untaxed_gain}, excluded: [symbol], max_stock_weight, warnings: [string], items: [{symbol, current_value, current_pct, target_pct, projected_value, projected_pct, price, diff_value, diff_shares, action_type, action_display, is_fund, is_etf, held, asset_class, sector, lots: [{lot_no, qty, bought, unit_cost, gain, long_term, long_term_on}], st_gain, lt_gain, tax, costs: {stt, stamp, exchange, brokerage, dp, fx_spread, total}, record, notes}]}` — `mode` is `rebalance`, `buy_only` or `raise`; `action_type` is `buy`, `sell` or `ok`; `diff_value` is negative for sells; with `--inflow` alone nothing is sold; empty and zero fields are left out | item |
| `portfolio target drift` | `{currency, total_value, absolute_band_pct, relative_band_pct, rows: [{key, kind, current_pct, target_pct, drift_pct, relative_drift_pct, band_pct, status}], warnings, excluded}` — `kind` is `symbol`, `sector`, `asset_class` or `untargeted`; `status` is `ok`, `over` or `under` | — |
| `verdict` | `{symbol, name, price, currency, overall_score, data_coverage, pillar_bands: {strong, moderate, weak, no_data}, pillars: [{name, score, weight, band, inputs_present, inputs_possible, summary}], note, bull_points: [text], bear_points: [text], next_catalyst, warnings: [text], disclaimer}` — `score` and `overall_score` are 0–100 and `null` when there is no data (then `band` is `no_data` and the pillar is left out of the composite); `band` is `strong` (65+), `moderate` (45–64), `weak` or `no_data`; `weight` is a share of 1; `data_coverage` is the share of input data present, in percent; `note` says why there is no composite | — |
| `score` / `scorecard` | `{symbol, name, currency, price, market_cap, sector, industry, piotroski: {score, max_score, evaluated, rating, rating_label, profitability_score, leverage_score, efficiency_score, criteria: [{id, category, name, formula, formatted_value, passed, points, explanation, evaluated}]}, altman: {z_score, zone, zone_label, formula_str, is_financial, note, missing_factors, components, z_double_prime, z_double_prime_zone, z_double_prime_components, z_double_prime_formula_str}, evaluation_date}` — `evaluated` counts the signals that had data (`score` out of `evaluated`, not always out of 9); `rating` is `null` unless all nine ran; `z_score` is `null` with the missing ratios in `missing_factors` | — |
| `options oi` | `{symbol, source, underlying, currency, expiry, expiries, lot_size, pcr: {call_oi, put_oi, call_volume, put_volume, oi_ratio, volume_ratio}, pcr_sentiment, max_pain, top_calls: [wall], top_puts: [wall], breakdown: {calls: {long_buildup_oi, short_covering_oi, short_buildup_oi, long_unwinding_oi, neutral_oi, long_buildup_count, ...}, puts: {...}}, calls_sentiment, puts_sentiment}`, wall = `{strike, open_interest, share_pct, oi_change, price_change, last_price, buildup}` — `buildup` is `long_buildup`, `short_covering`, `short_buildup`, `long_unwinding` or `null` (on screen `LB`, `SC`, `SB`, `LU`); `calls_sentiment` and `puts_sentiment` read each side for the underlying (put buying is bearish, put writing bullish); `pcr_sentiment`, `calls_sentiment` and `puts_sentiment` are words such as `Bullish` | — |
| `tax advance` | `{fy_label, fy_start, fy_end, as_of, total_stcg, total_ltcg, total_tax_estimated, sec_208_threshold, sec_208_applicable, total_paid, payments: [{date, amount}], assumed_on_time, ignored_before_year, total_sec_234c_interest, sec_234b_applicable, quarters: [{quarter, due_date, cutoff_date, stcg_quarter, ltcg_quarter, stcg_cumulative, ltcg_cumulative, cumulative_tax_estimated, required_cumulative_pct, min_cumulative_tax_required, quarter_tax_payable, paid, shortfall, sec_234c_interest, status}]}` — `paid` is what was paid on or before that due date; `status` is `paid`, `pending`, `upcoming`, `overdue` or `nothingdue` | quarter row |
| `goal sip` | `{target, years, months, annual_return_pct, effective_annual_pct, step_up_pct, savings, savings_future_value, monthly_sip, final_monthly_sip, total_invested, gains, lump_sum_today, schedule: [{year, monthly_sip, invested, value}], monthly_schedule: [{month, monthly_sip, invested, value, lots_long_term}], regime: {kind, name, return_pct, vol_pct, source}, glide, target_today, inflation_pct, real_value_today, real_value_cpi_pct, net_target, tax: {kind, slab_pct, exit_load_pct, short_term_lots, long_term_lots, short_term_gain, long_term_gain, exit_load, tax}, net_after_tax, monthly_sip_for_net_target, percentiles: {p10, p50, p90, success_prob_pct, basis, gross: {p10, p50, p90}, net}, sensitivity: [{return_pct, monthly_sip}], mismatch, tax_compare: [{kind, net_after_tax}], warnings: [text], goal, currency}` — `goal` is `sip`, `retire`, `education`, `swp`, `prepay`, `emergency`, `list` or `show`; `monthly_schedule` only up to 24 months; `glide`, `target_today`, `real_value_today`, `mismatch` and `tax_compare` only when they apply | — |
| `goal retire` | `{monthly_expense_today, years_to_retire, years_in_retirement, inflation_pct, return_pre_pct, return_post_pct, monthly_expense_at_retirement, monthly_income_today, monthly_need_at_retirement, buffer_years, legacy, corpus, initial_withdrawal_rate_pct, survival_years, crash_survival_years, swp_drag: {annual_withdrawal, gain_fraction, swp_tax, fd_tax}, sip: plan, goal, currency}` — `plan` is the first part of the `goal sip` object (up to `schedule`) | — |
| `goal education` | `{cost_today, years, months, inflation_pct, future_cost, sip: report, goal, currency}` — `report` is the full `goal sip` object | — |
| `goal swp` | `{corpus, cost_basis, monthly_withdrawal, inflation_pct, return_pct, years, fd_return_pct, slab_pct, initial_withdrawal_rate_pct, swp: path, fd: path, goal, currency}` — path = `{runway_months, lasts, final_value, total_withdrawn, total_tax}` | — |
| `goal prepay` | `{principal, loan_rate_pct, remaining_months, emi, extra_monthly, lump_sum, new_tenure_months, tenure_saved_months, interest_without_prepay, interest_saved, sip_return_pct, sip_tax, sip_net, prepay_equivalent, sip_percentiles, breakeven_return_pct, goal, currency}` | — |
| `goal emergency` | `{monthly_expense, months_cover, target, liquid, funded_pct, horizon_months, build: plan, goal, currency}` | — |
| `goal list` | `{goal, currency, goals: [{name, target, target_future, by, months_left, holdings, linked_value, funded_pct, monthly_sip, status}]}` — `status` is `funded`, `needs_sip` or `overdue` | — |
| `tax` | `{country, tax_year, currency, sales: [{symbol, qty, bought, bought_estimated, sold, gain, term}], holdings: [{symbol, cost, value, short_term_gain, long_term_gain, estimated_dates}], realized: estimate, if_sold_today: estimate, ltcg_exemption_left, unpriced: [symbol], other_currency: [symbol], equity_funds: [symbol], other_rules: [{symbol, reason}], intraday_sales: [{symbol, qty, bought, bought_estimated, sold, gain, term}], intraday_gain}`, estimate = `{short_term_gains, short_term_losses, long_term_gains, long_term_losses, taxable_short_term, taxable_long_term, exemption_used, short_term_tax, long_term_tax, cess, total_tax, loss_carried_forward}` — `term` is `short`, `long`, or null (UK); `ltcg_exemption_left` is India only; `equity_funds` (counted at listed-share rates), `other_rules` (left out: debt/other funds, SGBs) and `intraday_sales` (bought and sold on one day: slab-rate speculative income, left out of every estimate) are filled for India | open holding |
| `sim history` | `{stats: {days, winning_days, day_win_rate_percent, total_pnl, avg_daily_pnl, best_day, worst_day, trades, winning_trades, losing_trades, trade_win_rate_percent, sharpe, max_win_streak, max_loss_streak, charges, distinct_days, gross_trades}, confidence, sessions: [{date, market, capital, target_percent, settled, trades, pnl, pnl_percent, wins, losses}]}` — `days` counts sessions; day statistics group them by date (`distinct_days`). P&L is after charges except for the `gross_trades` settled before 0.9. `confidence` is `high`, `moderate`, `low`, or null before 20 dates and 30 trades | — |
| `trade selftest` | `{stockwise, os, date, market, broker, broker_name, experimental, account, secrets, orders_enabled, max_order_value_set, amounts_redacted, read_only, checks: [{name, status, detail}], shapes: [{endpoint, status, rows, missing, unknown, error}], summary: {pass, warn, fail, skip}}` — `status` is `pass`, `warn`, `fail` or `skip`; `account` is the last 2 characters only | — |
| `trade sync` | `{broker, experimental, portfolio, since, applied, orders: [{symbol, order_id, side, date, time, qty, price, fills, result, sales?, held?}], already_synced, skipped: [{order_id, symbol, reason}], open_orders: [{order_id, symbol, side, status, qty, filled_qty}], quantity_check: [{symbol, portfolio_qty, broker_qty}], gtt_changes: [change], notes}` — `result` is `lot`, `sold` (with `sales`) or `unmatched` (with `held`); `applied` is true only when `--apply` wrote something | — |
| `trade gtt list` | `{broker, experimental, gtts: [{id, symbol, side, qty, trigger, limit, status}], changes: [change], first_check, last_checked}`, change = `{kind, id, symbol, side, qty, trigger, from, to, state}` — `kind` is `new`, `status` or `gone`; `state` is `active`, `triggered`, `cancelled`, `expired`, `rejected` or `other` | — |
| `sim replay list` | `[{name, title, market, from, to, benchmark, benchmark_name, capital, stocks, description}]` | — |
| `sim replay status` | `{scenario, title, market, date, day, days, last_day, capital, cash, invested, equity, return_percent, max_drawdown_percent, costs_paid, cost_model, benchmark: {symbol, name, close, return_percent}, positions: [{symbol, sector, qty, avg_price, close, value, pnl, pnl_percent, weight_percent, stop, target, opened}], orders: [order], universe: [{symbol, sector, close, change_percent, change_20d_percent}], challenge, finished_challenges: [outcome], disclaimer}` — only data known at the replay date's close; `sim replay start` adds `description` and `skipped`. order = `{id, side, symbol, qty, stop, target, placed}`; challenge = `{name, title, description, day, days, started, rules: [{rule, goal, hard}], violations: [{date, rule, detail, hard}]}`; outcome = `{name, title, passed, reason, from, to, goals: [{goal, met, detail}], violations, sip}` | — |
| `sim replay next` | `{days: [{date, events: [event]}], new_badges, status}` — `event` is `fill` (`{date, symbol, side, qty, price, charges, reason, with_stop, weight_percent, pnl}`, `reason` = `order`, `stop-loss` or `target`), `contribution` (`{amount}`), `rejected` (`{symbol, side, qty, reason}`), `waiting` (`{symbol, side}`), `rule_broken` (a violation), `challenge_over` (an outcome) or `last_day` | — |
| `sim replay buy` / `sell` | `{order, warnings: [violation]}` — warnings are soft challenge rules the order breaks | — |
| `sim replay end` | `{kind, name, title, market, benchmark_name, from, to, finished, scorecard: {days, score, grade, return_percent, benchmark_return_percent, max_drawdown_percent, benchmark_max_drawdown_percent, sharpe, benchmark_sharpe, sortino, benchmark_sortino, exposure_percent, parts: {risk_adjusted, drawdown, discipline, diversification}}, badges, new_badges, disciplined, challenges: [{name, title, passed, reason}], curve, benchmark_curve, debrief, disclaimer}` — `score`/`grade` are null under 5 trading days; curves are rebased to 100 | — |
| `sim challenge list` | `[{name, title, description, days, modes, monthly_contribution, rules: [{kind, ...}]}]` — `days` 0 = until the replay ends | — |
| `sim challenge status` | `{mode, challenge, ...}` — `mode` `replay` adds `finished: [outcome]`; `live` adds `sessions, complete, return_percent, benchmark_return_percent, max_drawdown_percent, violations`; `challenge` is null when none runs | — |
| `sim challenge end` | replay: `{outcome, new_badges}`; live: the `sim replay end` shape plus `outcome` | — |
| `sim profile` | `{badges: [{id, title, how, earned, run}], streaks: {disciplined: {current, best}, challenges: {current, best}}, best: {NAME: {score, grade, date}}, challenges_passed, challenges_failed, runs: [run], disclaimer}` — `earned` is null for badges not earned yet; run = the `sim replay end` shape without `debrief`/`disclaimer` | — |
| `config show` | `{path, status, error, settings: [{key, value, source}]}` — `status` is `loaded`, `missing` or `invalid`; `source` is `flag`, `env`, `file` or `default` | — |

## Alert conditions

> **Guide:** [Set up alerts and the daemon](guide-alerts.md#2-add-alerts).

`stockwise alert add <SYMBOL> <CONDITION> <NUMBER>`:

| Condition | NUMBER | Fires when |
|-----------|--------|------------|
| `above` / `below` (`>` / `<`) | price | the live price is at or beyond the level |
| `rsi-below` / `rsi-above` | RSI level (0–100) | RSI (14-day, or `--period N`) is at or beyond the level |
| `cross-above-sma` / `cross-below-sma` | days (2–400) | the daily close crosses its simple moving average — on the crossing day only |
| `cross-above-ema` / `cross-below-ema` | days (2–400) | the same with an exponential moving average |
| `breakout-high` / `breakdown-low` | days (2–1000) | the close is above the highest high (below the lowest low) of the previous N sessions |
| `change-above` / `change-below` | percent | the day's change is +N% or more / −N% or less (`change-below 3` and `change-below -3` are the same) |
| `volume-spike` | multiple (1–100) | today's volume is at least N times its 20-day (`--period`) average |
| `results-in` | days (0–90) | the next results date is at most N days away (from [`calendar`](#results-and-corporate-calendar)) |

Technical conditions read daily bars (one chart request per symbol per
check), with the last bar updated to the live quote, so a cross or breakout
fires on the session it happens in. A symbol with too little history reads
`no_data` and never triggers. `alert watch` notifies each alert once until it
re-arms, at most once per trading day.

```bash
stockwise alert add INFY rsi-below 30
stockwise alert add INFY rsi-above 70 --period 9
stockwise alert add RELIANCE cross-above-sma 200
stockwise alert add AAPL -m us breakout-high 52
stockwise alert add HDFCBANK change-below 3
stockwise alert add TCS volume-spike 2
stockwise alert add TCS results-in 7
```

Alerts saved by earlier versions keep working unchanged.

Options of `alert add`:

- `--on-close` (crosses and breakouts): judge completed sessions only, so an
  intraday poke through the average that closes back does not notify.
- `--cross` (price levels): notify only when the price crosses the level; an
  alert added while the price is already beyond it waits for it to come back
  and cross. Without it, a level alert notifies while the level holds (once a
  day) and re-arms once it stops holding.
- An alert identical to an existing one (same symbol, condition, number,
  period and options) is refused.

`alert watch` and the worker check price levels only on fresh quotes (traded
within 30 minutes) while the symbol's market is open; crosses on sessions
completed while nothing was running are caught up once (up to 10 sessions).
Each alert has a stable id in `alerts.json`; the notified state is kept per
id, so removing or editing one alert never re-sends another.

## What's hot and today

`stockwise today` answers "what changed in my portfolio today, and why?".
For every holding: the day move, the day P&L in the portfolio's base
currency, the move in excess of the benchmark (NIFTY 50 or the S&P 500) in
units of the stock's own excess-return volatility (σ), the volume against its
20-day average, and a one-line reason:

- the most important fresh headline or filing (NSE/BSE announcements, SEC
  EDGAR 8-K and the like are preferred over news);
- "with the market" for an ordinary move in line with the benchmark;
- "unexplained" for an unusual move — 2.5σ or more (and at least 3%) against
  the benchmark, or 1.5% on 3× volume, the same test `events` uses — with no
  news or filing found.

The news feed merges headlines and filings for holdings and the watchlist,
removes duplicates, and ranks them by portfolio weight × importance
(category, filing or not, and recency halving every 24 hours).

`stockwise hot` ranks holdings and watchlist names by **heat** = |excess
move| in σ (capped at 6) + volume surge (multiple − 1, capped at 4) + the
importance of fresh news (up to 4) + 1.5 for a date within three days,
multiplied by 1 + 5 × portfolio weight. Names with no heat are left out. Each
row shows the move, the why and the holding's [action](#actions).

The market part scans the universe's quotes (India: the liquid NIFTY 500
stocks or `--universe`; US: the S&P 1500) with a liquidity floor: biggest
moves, unusual volume (RVOL against the 3-month average, paced for the time
of day while the session is open, at least 2×), gaps of 3% or more (4% with
`--cap small`), 52-week highs and lows, the stocks most mentioned in today's
business headlines, sector indices or ETFs, theme baskets (AI,
semiconductors, defence, battery and EV, minerals and metals; equal-weight),
cross-market movers among Indian and US large caps, and for India the
FII/DII flows, NSE bulk and block deals, the F&O ban list and open-interest
build-up. A source that fails is named at the end; the rest still show.

```bash
stockwise today
stockwise today --days 3 --json
stockwise hot
stockwise hot --cap small
stockwise hot -m us --json
```

## Holding monitor and digest

The background worker (`stockwise worker start`, or `worker install` for a
login service) watches every holding with no setup:

| Signal | Default | When |
|--------|---------|------|
| Day drop | 5% or 2σ (and ≥ 1.5%) | during the session (every 15 minutes) |
| New 52-week low | below the year's low | during the session |
| Gap at the open | 3% either way | during the session |
| Below cost | 20%, then every further 10% | after the close |
| Off the 52-week high | 25%, then every further 10% | after the close |
| Dates tomorrow | results, ex-dividend, record date, split, bonus | after the close |
| Promoter pledge / shareholding | a new pledge; promoter ±1 point; FII+DII ±2 points | after the close (a few holdings per day) |
| Analyst rating | upgrade or downgrade in the last 3 days | after the close |
| Insider trade | NSE PIT / Form 4 in the last 3 days | after the close |

Each signal is sent once. Signals during the session go out at once, several
together as one message; after the close everything — signals, the LTCG,
results and filing notices, alerts that triggered today — goes out as **one
digest** with the day's P&L, the top contributors and detractors with the
news or filing behind each move (or "unexplained") and the dates coming up.
`daemon longterm` sends the same digest with its findings in it.

Thresholds (`[monitor]` in the config file, or `STOCKWISE_MONITOR_<KEY>`):
`enabled`, `day_drop_pct`, `day_drop_sigma`, `from_cost_pct`,
`from_peak_pct`, `gap_pct`, `digest`.

Notifications have a priority (low, normal, high). `STOCKWISE_NOTIFY_ROUTES`
sends a channel only some of them (`desktop=high,telegram=low`), and
`STOCKWISE_QUIET_HOURS` (`22:00-07:00`, local time) holds everything below
high priority until the quiet hours end, then sends it as one message.
Channels: Telegram, Slack, Discord, email, ntfy (`STOCKWISE_NTFY_TOPIC`,
`STOCKWISE_NTFY_SERVER`, `STOCKWISE_NTFY_TOKEN`), Pushover
(`STOCKWISE_PUSHOVER_TOKEN`, `STOCKWISE_PUSHOVER_USER`) and desktop.

## Results and corporate calendar

> **Guide:** [Check a stock or fund fast](guide-research.md#5-see-what-happened-and-what-the-data-says) and [Set up alerts and the daemon](guide-alerts.md).

`stockwise calendar` lists what is coming up for your holdings and watchlist
(`--holdings` or `--watchlist` for one of them, or name symbols), over the
next 30 days (`--days 1-365`):

| Source | Covers | Events |
|--------|--------|--------|
| Yahoo Finance (`quoteSummary` calendarEvents) | US and Indian stocks | results (earnings) date or expected window with the EPS estimate, ex-dividend date, dividend payment date |
| NSE board meetings | `.NS` / `.BO` stocks (looked up by their NSE symbol) | board meetings; those that consider results are listed as results |
| NSE corporate actions | `.NS` / `.BO` stocks | ex-dates of dividends, splits, bonus and rights issues, other actions |

- For an Indian stock, NSE's filed results date replaces Yahoo's estimate,
  and NSE's ex-dividend date replaces Yahoo's.
- Upcoming splits come from NSE only; Yahoo lists splits after they happen,
  so US splits don't appear in advance.
- BSE filings are not read; a `.BO` symbol uses the same company's NSE
  filings.
- NSE's API serves browser sessions. StockWise opens one session per run,
  waits a second between requests and caches every answer for a day in
  `calendar_cache.json` (failures for an hour). When NSE refuses, the table
  says so and shows Yahoo's dates.
- Crypto, FX and indices have no calendar and are skipped.

```bash
stockwise calendar
stockwise calendar --holdings --days 14
stockwise calendar TCS INFY
stockwise calendar AAPL MSFT -m us --json
```

## Market and portfolio context

`stockwise use` picks the market bare tickers belong to and the portfolio
commands read, and saves both in the [configuration file](#configuration-file)
(`market` and `portfolio`), the same way `stockwise init` saves the market.

```bash
stockwise use                       # what is in use, and where each value comes from
stockwise use us                    # bare tickers are US stocks from now on
stockwise use long-term             # work on the `long-term` portfolio
stockwise use us-tech us --bind     # bind a portfolio to a market and switch to both
stockwise use us-tech               # later: picks the portfolio and its market
stockwise use us-tech --unbind      # remove the binding
```

A market word (`in`, `us`, `uk`, `crypto`, ...) is the market; anything else
is a saved portfolio's name (`default` is `portfolio.json`). Bindings live in
the config file under `[portfolio_markets]`, not in the portfolio file.

The market for one command comes from the first of these that is set:

1. `-m` / `--market`
2. `STOCKWISE_MARKET`
3. the market bound to a portfolio picked with `--portfolio` or `STOCKWISE_PORTFOLIO`
4. the config file's `market` (what `stockwise use` saves)
5. the market bound to the config file's portfolio, when no portfolio was picked for this run (a portfolio picked with `--portfolio` and no binding of its own skips this step)
6. `in`

**No `-m` needed.** When the market comes from steps 3–6, a symbol that says
where it trades is looked up there, offline:

- an exchange suffix or index: `VOD.L` → London, `TCS.NS` → India, `^GSPC` → US, `BTC-USD` → crypto;
- an ISIN: well-known ones map to their ticker (`US0378331005` → `AAPL`,
  `IE00BK5BQT80` → `VWCE.DE`); others pick the market from the country prefix
  (`IN` → India, `US` → US, `GB` → London, `DE`, `JP`, `HK`, `SG`, `CA`, `AU`),
  and `stockwise search <ISIN>` finds the ticker;
- a bare ticker that only one market's built-in list holds (S&P 1500, NIFTY 500,
  the [global index lists](markets.md), the large coins, `GOLD`/`SILVER`):
  `stockwise NVDA` with India as the default quotes NVIDIA and says so on stderr.

A ticker listed in several markets (`RIO` in London and Sydney) stays on the
current market, with a one-line "did you mean" hint naming the others. Numeric
codes (BSE scrip codes, Tokyo and Hong Kong codes) always use the current
market. With several symbols, the market moves only when they all agree.

## Configuration file

Everything works without one. To set your own defaults:

```bash
stockwise config init     # write a commented template
stockwise config path     # where it is
stockwise config show     # every effective setting and where it came from
```

The file is `config.toml` in your platform's config directory —
`~/Library/Application Support/stockwise/` on macOS, `~/.config/stockwise/`
on Linux (`$XDG_CONFIG_HOME`), `%APPDATA%\stockwise\` on Windows — or the
file named by `STOCKWISE_CONFIG`.

```toml
market = "us"                       # in | us | crypto | fx | commodity | uk | de | jp | hk | sg | ca | au
lang = "hi"                         # en | hi
quote_currency = "inr"              # BTC → BTC-INR with -m crypto
portfolio = "long-term"             # a named portfolio
ai = false
ai_provider = "ollama"              # ollama | openai | anthropic | gemini
model = "llama3.1:8b"
ollama_host = "http://gpu-box:11434"
data_provider = "yahoo"             # yahoo | alphavantage | massive | twelvedata
redis_url = "off"
home = "~/Dropbox/stockwise"        # data directory; ~ is your home

[periods]                           # default -p per command
technical = "6mo"
backtest = "5y"
sip = "10y"

[notify]                            # STOCKWISE_* notification variables, lower-case, no prefix
telegram_chat_id = "123456789"
smtp_host = "smtp.gmail.com"
email_to = "me@example.com"

[portfolio_markets]                 # market per named portfolio (`stockwise use NAME MARKET --bind`)
us-tech = "us"
```

| Key | Overridden by | Built-in default |
|-----|---------------|------------------|
| `market` (`stockwise use`) | `-m`, `STOCKWISE_MARKET` | `in` |
| `ai` | `--ai` / `--no-ai`, `STOCKWISE_AI` | `false` |
| `model` | `STOCKWISE_MODEL` | `qwen3:14b` |
| `ollama_host` | `OLLAMA_HOST` | `http://localhost:11434` |
| `redis_url` | `STOCKWISE_REDIS_URL` | `redis://127.0.0.1:6379/` |
| `home` | `STOCKWISE_HOME` | platform data directory (see `doctor`) |
| `lang` | `--lang`, `STOCKWISE_LANG` | your locale, else `en` |
| `quote_currency` | `--quote-currency`, `STOCKWISE_QUOTE_CURRENCY` | `usd` |
| `portfolio` | `--portfolio`, `STOCKWISE_PORTFOLIO` | `default` |
| `[portfolio_markets] <portfolio>` | `-m`, `STOCKWISE_MARKET` | none; see [Market and portfolio context](#market-and-portfolio-context) |
| `ai_provider` | `STOCKWISE_AI_PROVIDER` | `ollama` |
| `data_provider` | `STOCKWISE_DATA_PROVIDER` | `yahoo` |
| `[periods] <command>` | `-p` | per command, see `<command> --help` |
| `[notify] <key>` | `STOCKWISE_<KEY>` | none (see [notifications](bots.md#notifications)) |

`[periods]` accepts the commands that take `-p`: `deep`, `technical`,
`history`, `risk`, `patterns`, `support`, `fibs`, `volume`, `gaps`,
`stoploss`, `forecast`, `options`, `backtest`, `sip`. `--help` shows the
default from your file, e.g. `[default: 6mo]`.

A file that doesn't parse — an unknown key, a wrong type, an unknown market or
period — stops every command with exit code 1 and an error naming the file and
the key. `config`, `use`, `doctor` and `completions` still run, so you can inspect it
or replace it with `stockwise config init --force`.

API keys (`OPENAI_API_KEY`, the data provider's key, ...) and
`STOCKWISE_API_TOKEN` are read from the environment only. Notification tokens
and passwords may go in `[notify]`; if they do, keep the file private
(`chmod 600`) — `config show` and `doctor` warn when other users can read it.

`stockwise serve` answers with the same shapes over HTTP — see
[server.md](server.md).
