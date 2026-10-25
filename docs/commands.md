# Commands Reference

`stockwise --help` lists every command by group; `stockwise <command> --help`
shows its arguments. This page adds detail.

## Global options

| Option | Env var | Description |
|--------|---------|-------------|
| `-m, --market <MARKET>` | `STOCKWISE_MARKET` | Market for bare tickers. Default `in`: `TCS` → `TCS.NS`. `us`: `AAPL`. `crypto`: `BTC` → `BTC-USD`. `fx` (or `forex`): `USDINR`, `usd/inr` → `USDINR=X`. `commodity`: `GOLD` → `GC=F`, `SILVER` → `SI=F`, other bare tickers → NSE (`GOLDBEES` → `GOLDBEES.NS`). Global exchanges from the [market table](markets.md): `uk` (`VOD` → `VOD.L`), `de` (`SAP.DE`), `jp` (`7203` → `7203.T`), `hk` (`5` → `0005.HK`), `sg` (`D05.SI`), `ca` (`RY.TO`), `au` (`BHP.AX`). Symbols with a suffix (`RELIANCE.BO`, `VOD.L`), indices (`^NSEI`), FX (`USDINR=X`), futures (`GC=F`) and crypto pairs (`BTC-INR`) pass through unchanged |
| `--quote-currency <CODE>` | `STOCKWISE_QUOTE_CURRENCY` | Currency bare crypto tickers are quoted in with `-m crypto`. Default `usd`; `inr` gives `BTC-INR` |
| `--lang <en\|hi>` | `STOCKWISE_LANG` | Language for help groups, tables and messages. Default: your locale (`LC_ALL` / `LC_MESSAGES` / `LANG` starting with `hi` picks Hindi), else English. `--json` and `--csv` output is never translated |
| `--ai` | `STOCKWISE_AI` | Add an AI section to `analyze`, `technical`, `compare`, `screen`, `backtest`, `deep` (slow with a local model; see [ai.md](ai.md)) |
| `--no-ai` | | Turn the AI section off for one run, even if `STOCKWISE_AI` or the config file turn it on |
| `--portfolio <NAME>` | `STOCKWISE_PORTFOLIO` | Use a named portfolio (`portfolios/NAME.json`) instead of the default `portfolio.json`. Applies to `portfolio`, `import`, `tax`, `wealth`, `dashboard`, `trade holdings` and every other command that reads holdings |

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

**Machine-readable output**: commands marked `[--json]` print JSON, and
`[--json\|--csv]` print JSON or CSV, instead of a table. See
[Scripting](#scripting-with---json-and---csv).

## Start here

| Command | Description |
|---------|-------------|
| `quote <SYMBOLS...> [--json\|--csv]` (`q`) | Price, change, day/52-week range, volume, market cap, P/E, EPS, moving averages. Unknown symbols are named in the error |
| `deep <SYMBOL> [-p PERIOD]` | Everything about one stock: fundamentals, technicals, support/resistance, Fibonacci, risk, gaps, volume, insights. Default `3mo` |
| `markets [--crypto] [--fx] [--global] [--json\|--csv]` | US + India indices (S&P 500, Dow, NASDAQ, Russell, VIX, NIFTY 50, SENSEX, Bank NIFTY, NIFTY IT) and the India 10-year G-sec yield (CCIL, RBI fallback; left out when neither answers); `--crypto` adds the largest coins, `--fx` major pairs (USD/INR, EUR/USD, ...), `--global` one row per exchange's main index (NIFTY 50, S&P 500, FTSE 100, DAX, Nikkei 225, Hang Seng, Straits Times, S&P/TSX, S&P/ASX 200) with whether it is open and its local time. With `-m crypto`, `-m fx`, `-m commodity` or a global exchange that section comes first. `--json` rows carry `region`: `Indian Markets`, `US Markets`, `Crypto`, `FX`, `Commodities`, a country (`United Kingdom`, `Japan`, ...) or `Global` |
| `dashboard` (`d`) | Indices + portfolio P&L + watchlist + triggered alerts; first-run hints when empty |
| `search <QUERY> [--json\|--csv]` | Find a ticker by company name |
| `doctor` | Check the config file, market and language, data directory, Yahoo reachability, the data provider that answered, Redis, AI provider and model (never the key), broker config. Exits 1 if market data is unreachable or the config file is invalid |

## Analysis

| Command | Description |
|---------|-------------|
| `analyze <SYMBOL>` (`a`) | Valuation, profitability, growth, analyst ratings, plain-English verdict |
| `technical <SYMBOL> [-p] [--json]` (`t`) | RSI, MACD, Bollinger Bands, ATR, VWAP, SMA/EMA, braille charts. Default `3mo` |
| `history <SYMBOL> [-p] [--json\|--csv]` | Braille price chart, volume bars, last 20 bars. `--json`/`--csv` emit every OHLCV bar. Default `1mo` |
| `chart <SYMBOL>` | 1W, 1M, 3M and 1Y charts at once |
| `compare <SYMBOLS...> [--json\|--csv]` (`c`) | Two or more stocks side by side across 16 metrics |
| `risk <SYMBOL> [-p] [--json]` | Annualized volatility, Sharpe/Sortino/Calmar, max drawdown, VaR 95/99% — always from daily bars. Default `1y` |
| `correlate <SYMBOLS...> [--json\|--csv]` (`corr`) | Correlation matrix of daily returns (two or more symbols) |
| `timeframes <SYMBOL>` (`mtf`) | 1M–5Y return, volatility, RSI, SMA/MACD signals |
| `returns <SYMBOL> [--json\|--csv]` | 1W–5Y returns with CAGR |
| `patterns <SYMBOL> [-p]` | Candlestick patterns (doji, hammer, engulfing, morning star) |
| `support <SYMBOL> [-p]` | Pivot points (R1–R3, S1–S3) + Fibonacci levels |
| `fibs <SYMBOL> [-p]` | Fibonacci retracement with a "you are here" marker |
| `volume <SYMBOL> [-p]` (`vol`) | OBV, A/D line, volume ratio, volume bars |
| `gaps <SYMBOL> [-p]` | Filled and unfilled price gaps |
| `stoploss <SYMBOL> [-e PRICE] [-p]` | ATR (1.5x/2x/3x), percentage and Chandelier stops, R:R targets |
| `forecast <SYMBOL> [-d DAYS] [-p]` | Linear trend projection with 95% bands — a projection, not a prediction |

## Market

| Command | Description |
|---------|-------------|
| `movers [--universe INDEX] [--json\|--csv]` | Top gainers and losers among 30 large caps, or an NSE index's members (see below) |
| `sectors [--json\|--csv]` | Sector ETF/index performance with bars |
| `heatmap [--universe INDEX]` | Heatmap of the 30-stock universe, or an NSE index's members |
| `sentiment` | Fear & Greed style gauge from VIX, momentum and trend |
| `news <SYMBOL>` | Headlines collated from Yahoo Finance and Google News |
| `rotation` | Sector rotation vs 50-day average |
| `sectorcmp <SECTOR1> <SECTOR2>` | Two sectors head to head |
| `peers <SYMBOL>` | Same-sector peers on P/E, P/B, market cap |
| `insider <SYMBOL>` | Analyst consensus + insider-related news |
| `ipo` | IPO calendar and news |
| `earnings` | Earnings growth for portfolio and watchlist |
| `surprise` | Earnings momentum — accelerating vs decelerating |
| `matrix` | Quality vs growth quadrants |
| `picks` | Large caps ranked on 8 fundamental + technical factors |
| `screen <CATEGORY> [--universe INDEX] [--json\|--csv]` | `undervalued` (P/E<15), `growth` (forward EPS > trailing), `dividend` (yield>2%), `momentum` (top gainers), `bluechip` (>$100B / >₹5L Cr) |
| `dividends <SYMBOL> [--json]` | Yield, income projection, 10-year DRIP growth |

**`--universe`** (Indian market): scan the current members of an NSE index
instead of the built-in 30 large caps — `nifty50`, `niftynext50` (`next50`),
`nifty100`, `banknifty` (`niftybank`) or `niftyit`. The member list comes from
NSE's website (one request) whichever data provider you use; prices still come
from your provider, and with `STOCKWISE_DATA_PROVIDER=nse` or `exchange` the
members' prices arrive with that same request. When NSE can't be reached or
refuses, StockWise says so on stderr and scans a built-in list instead (the 30
large caps for `nifty50` / `nifty100`, a built-in member list for `banknifty`
and `niftyit`); `niftynext50` has no built-in list and fails with the reason.

## Funds & F&O

| Command | Description |
|---------|-------------|
| `mf search <QUERY...> [-n N] [--json]` | Find Indian mutual-fund schemes by words in the name, fund house, category or ISIN; add `direct growth` to narrow. Shows the AMFI scheme code the other `mf` commands take |
| `mf nav <CODES...> [--json]` | Latest NAV and date, fund house, category, ISINs, and the plan's expense ratio (TER) with the other plan's for comparison |
| `mf history <CODE> [-p PERIOD] [--json]` | NAV chart for the period (default `1y`), expense ratio (TER), trailing 1M–10Y returns (annualized from 1Y), since-inception CAGR, max drawdown. `--json` includes every NAV in the period |
| `mf sip <CODE> <AMOUNT> [-p PERIOD] [--json]` | Monthly SIP from NAV history (default `5y`): value-vs-invested chart, gain, units, and XIRR on the actual purchase dates |
| `mf screen [CATEGORY...] [--plan direct\|regular\|all] [-s 1y\|3y\|5y\|ter] [--max-ter PERCENT] [-n N] [--max N] [--json]` | Bare: list categories with their active scheme counts. With words (`flexi cap`, `elss`, `liquid`, `equity etf`): rank the category's growth-option schemes by return (default 3Y CAGR, direct plans) or by expense ratio (`-s ter`, lowest first — with more matches than `--max`, the cheapest are screened), with a TER column. `--max-ter 1.0` keeps only funds whose disclosed TER is at most 1% |
| `mf overlap <A> <B> [--json]` | Shared top holdings, holdings and sector overlap, and expense ratios of two funds or ETFs, e.g. `mf overlap SPY QQQ -m us` |
| `options chain <SYMBOL> [-e DATE] [-n STRIKES] [--rate %] [--div-yield %] [--json]` | Calls and puts around the money: last price, open interest (and change on NSE/BSE), volume, IV, delta, gamma, theta, vega, rho, and the lot size. Default: nearest expiry, 10 strikes each side (`-n 0` for all) |
| `options pcr <SYMBOL> [-e DATE] [--json]` | Put/call ratio by open interest and by volume, and the three strikes with the most call and put open interest |
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

| Command | Description |
|---------|-------------|
| `backtest <SYMBOL> [-s STRATEGY] [-P NAME=VALUE] [-p] [--json\|--csv]` | Backtest one stock after costs: net and gross return, buy & hold, CAGR, drawdown, Sharpe/Sortino, exposure, trades. Fills at the next bar's open. Strategies: `rsi` (default), `macd`, `sma`, `bb`, `vwap`, `mr`, or your own TOML file. Default `2y` |
| `backtest portfolio <SYMBOLS...> [-s] [--weights equal\|inverse-vol\|0.5,0.3,...] [--rebalance none\|monthly\|quarterly] [--json\|--csv]` | One strategy across several stocks sharing the capital; combined equity curve and per-symbol contribution |
| `backtest walkforward <SYMBOL> [-s] [--train 1y] [--test 3mo] [--json\|--csv]` (`wf`) | Tunes the strategy's `[grid]` on rolling train windows and reports only the unseen test windows, with an overfitting verdict. Default `5y` |
| `backtest strategies [FILES...] [--json]` | List built-in and your own strategies, or check strategy files (exit 1 on errors) |
| `sip <SYMBOL> <AMOUNT> [-p] [--json]` | Monthly SIP/DCA simulation with value-vs-invested chart and XIRR. Default `5y` |
| `whatif <SYMBOL> <AMOUNT> <YYYY-MM-DD> [--json]` | What a lump sum on a past date is worth today |
| `intraday <CAPITAL> <TARGET%> [-s STRATEGY]` (`bot`) | 9-strategy intraday ideas with Kelly sizing and dual targets. `-s` adds a strategy file's rules as a tenth signal. Signals only |
| `longterm [MONTHLY_BUDGET]` | 6-pillar scoring, Monte Carlo, SIP allocation |
| `daemon intraday [CAPITAL]` | Paper-trading loop on the NSE clock (IST, holidays included; `-m us` uses NYSE hours in ET): scan, enter at the open, trail stops every 60s, book half at T1, square off 15 minutes before the close, settle into `sim` history. Survives network blips, resumes the day after a restart, and squares off on Ctrl+C. **Never sends broker orders** |
| `daemon longterm` | Post-market scan: scores, portfolio check, alerts, tax-harvest ideas |
| `sim start [CAPITAL] [TARGET%]` | Start a paper session with the bot's picks (default ₹25,000, 2%) on a trading day before the close |
| `sim status` / `history [--json]` | Live P&L / record + confidence rating |
| `sim settle [--now]` | Settle open sessions from that day's 5-minute bars: stop or target first, else exit at the 15:15 bar. Today's session waits for the close unless `--now` |
| `sim reset [--yes]` | Delete simulator history after confirmation (a timestamped backup is kept) |
| `sim replay list [--json]` (`scenarios`) | Built-in historical replays: `covid-crash-2020`, `gfc-2008`, `adani-2023`, `dotcom-2000`, `bull-run-2021`, `rate-hikes-2022`. See [simulator.md](simulator.md) |
| `sim replay start SCENARIO\|file:PATH.toml [--capital] [--costs] [--json]` | Start a replay: daily history is fetched once, then you trade day by day and only ever see bars up to the replay date |
| `sim replay start --from DATE --to DATE --symbols A,B,... [--benchmark SYM]` | Your own replay window in the active market (`-m in`/`-m us`); benchmark NIFTY 50 / S&P 500 by default |
| `sim replay buy SYMBOL QTY [--stop P] [--target P]` / `sell SYMBOL [QTY]` | Queue an order for the next day's open (backtest cost models; long only). `sell` without a quantity sells the whole position |
| `sim replay next [N]` / `status` / `chart SYMBOL\|index [--days]` | Move N trading days (stops and targets on each day's range); account, positions, orders and the market on the replay date; a chart up to it |
| `sim replay cancel [ID]` / `set SYMBOL [--stop P] [--target P]` | Cancel pending orders; move a position's stop or target |
| `sim replay end [--json]` | Finish: 0–100 score and grade, badges, streaks, and what actually happened |
| `sim challenge list\|start NAME\|status\|end [--json]` | Goal-based challenges with hard and soft rules over the running replay or your live paper sessions: `beat-the-index`, `steady-hands`, `position-limit`, `stop-loss`, `sip-discipline`, `crash-survivor`, or `file:PATH.toml` |
| `sim profile [--json]` | Badges (earned and how to earn the rest), streaks, best score per scenario, recent runs |
| `sim card [--svg FILE] [--text]` | Your last run as a text card, or a 640 × 360 SVG image — with the paper-trading disclaimer |
| `report <intraday\|longterm\|portfolio\|SYMBOL>` | AI report via local Ollama |

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

Data lives in the directory shown by `stockwise doctor`.

| Command | Description |
|---------|-------------|
| `portfolio [show] [--base inr\|usd] [--json\|--csv]` (`p`) | Holdings with live P&L, XIRR and allocation; holdings without a live price show N/A. Mixed INR/USD holdings are totalled in `--base` (default: the market's currency) |
| `portfolio add <SYMBOL> <SHARES> <COST> [--date YYYY-MM-DD]` | Add a purchase as a new lot (default date: today) |
| `portfolio sell <SYMBOL> <SHARES> [PRICE] [--date YYYY-MM-DD]` | Record a sale, oldest lots first (FIFO). Price defaults to the live price; a past `--date` needs a price. Refused while a split or bonus before the sale isn't applied to your lots |
| `portfolio lots <SYMBOL> [--lot N --date YYYY-MM-DD]` | Lots, sales, dividends and XIRR of one holding; `--lot N --date` corrects a purchase date |
| `portfolio actions [SYMBOL] [--apply]` | Splits, bonuses and dividends since your purchases, including positions you have sold (from Yahoo). Shows them; `--apply` adjusts lots and records dividends, and is safe to repeat |
| `portfolio list` | Your portfolios: the default one and every named one |
| `portfolio benchmark [NIFTY\|SENSEX\|SP500\|...] [-p 1y] [--base inr\|usd] [--json\|--csv]` | Time-weighted return against an index over the period, from the lots held each day: alpha, beta, tracking error, and attribution by holding and sector (see [below](#benchmark-and-attribution)) |
| `portfolio remove <SYMBOL>` | Remove a holding and its lots without recording a sale |
| `watch [show] [--json\|--csv]` / `watch add <SYMBOLS...>` / `watch remove <SYMBOLS...>` (`w`) | Watchlist |
| `alert [list] [--json\|--csv]` / `alert add <SYMBOL> <CONDITION> <NUMBER> [--period N]` / `alert check [--json\|--csv]` / `alert remove <N>` | Price and technical alerts — conditions below; `alert list` shows each condition in words |
| `alert watch [--interval 60] [--once]` | Check alerts while their market is open and notify each trigger once (see [notifications](bots.md#notifications)). `--once` checks every alert once and exits, for cron |
| `calendar [SYMBOLS...] [--days 30] [--holdings\|--watchlist] [--json\|--csv]` | Upcoming results, ex-dividend and dividend payment dates, splits and bonus issues for your holdings and watchlist (or the named symbols), sorted by date, with today and the next 7 days marked. See [the calendar](#results-and-corporate-calendar) |
| `import <kite\|indmoney\|groww\|upstox\|cas\|csv> <FILE> [--replace]` | Import broker holdings, a lot list, a tradebook or a CAS (casparser JSON/CSV); re-importing updates instead of doubling (see below and [brokers.md](brokers.md#importing-holdings)) |
| `export <portfolio\|watchlist>` | Write a CSV to the current directory |
| `wealth` | Save today's portfolio value and show growth (run daily) |
| `rebalance` | Trades to reach equal weight |
| `harvest` | Tax-loss harvesting candidates |
| `tax <in\|us\|uk> [--json\|--csv]` | Capital-gains estimate for the current tax year: realized sales and unrealized lots, each with its own holding period (see below) |
| `tax export [in\|us] [--fy 2025-26] [--out FILE] [--json]` | Capital-gains schedule of a tax year's sales: an ITR Schedule CG / 112A CSV for India, a Form 8949 worksheet for the US (see [below](#tax-export)) |
| `divcal` | Dividend ranking and income for portfolio + watchlist |
| `pcorr` | Correlation between holdings, diversification warnings |

### Lots, sales and tax

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
but not included. It is an estimate — no cess, surcharge, grandfathering or
brought-forward losses.

### Tax export

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
or before 31 Jan 2018 are flagged — Schedule 112A lets you use that day's
fair market value as the cost, which StockWise doesn't apply; sales before
23 Jul 2024 are noted because they had the old rates (15% / 10%); mutual fund
rows are noted because only equity-oriented funds belong in 111A/112A; sales
in another currency (US shares for `in`) and crypto (taxed at 30% under
section 115BBH, Schedule VDA) are listed but left out. Years before FY 2024-25
are noted too: their LTCG exemption was ₹1 lakh.

### Benchmark and attribution

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
remembers the splits applied to it, so running it again is a no-op. Indian tax
treats bonus shares as bought at zero cost on the allotment date — check bonus
lots against your tax statement.

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
connection). A symbol is left unchanged when its sells have no earlier buy in
the file, or when it already has lots or sales from elsewhere (manual entries,
another import) — `--replace` rebuilds it from the file anyway. Dates may be
`YYYY-MM-DD`, `DD-MM-YYYY`, `DD/MM/YYYY` (`MM/DD/YYYY` with `-m us`) or
`15-Mar-2024`; a file with an unrecognized date is refused.

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
| `goal sip --target AMOUNT --years N [--return 12] [--step-up PCT] [--savings AMOUNT] [--json]` | Monthly SIP that reaches the target, with an optional yearly step-up; total invested, gains, the lump sum that would do instead, and a year-by-year table |
| `goal retire --monthly-expense AMOUNT --years-to-retire N --years-in-retirement N [--inflation 6] [--return-pre 12] [--return-post 7] [--step-up PCT] [--savings AMOUNT] [--json]` | Corpus needed on the day you retire to pay today's expenses (grown by inflation) for every month of retirement, and the SIP that builds it |
| `goal education --cost AMOUNT --years N [--inflation 8] [--return 12] [--step-up PCT] [--savings AMOUNT] [--json]` | Today's cost after N years of education inflation, and the SIP that pays it |

```bash
stockwise goal sip --target 1cr --years 15                  # ₹19,819 a month at 12%
stockwise goal sip --target 1cr --years 15 --step-up 10     # ₹11,516 a month, rising 10% a year
stockwise goal retire --monthly-expense 80k --years-to-retire 20 --years-in-retirement 30
stockwise goal education --cost 25L --years 10 --savings 3L
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

Both use the same read-only tools (quotes, history, technicals, risk, mutual
funds, option chains, screens, backtests, your portfolio, lots, tax estimate,
calendar, goals, watchlist and alerts). See [ai.md](ai.md).

| Command | Description |
|---------|-------------|
| `ask <QUESTION> [--verbose] [--max-steps 6] [--max-tokens 100000] [--json]` | Ask in plain English; the configured model (local Ollama by default, or the cloud provider in `STOCKWISE_AI_PROVIDER`) calls the tools and answers. Needs a model with tool calling. `--verbose` prints each tool call on stderr; `--json` prints the transcript (answer, tool calls with arguments, token usage). The last request runs without tools, so the model must answer. Educational, not financial advice |
| `mcp [--http ADDR] [--allow-remote] [--allow-orders] [--list-tools]` | [Model Context Protocol](https://modelcontextprotocol.io) server for Claude Desktop, Claude Code, Cursor and other assistants: 24 read-only tools, the portfolio, watchlist and alerts as resources, and three prompts. Stdio by default; `--http 127.0.0.1:8788` serves Streamable HTTP at `/mcp` (other addresses need `--allow-remote` and `STOCKWISE_API_TOKEN`). `--allow-orders` adds a dry-run `order_preview` that returns the `stockwise trade` command for you to run — it never places orders. `--list-tools` prints the tools and their input schemas as JSON. Cargo feature `mcp` (on by default) |

```bash
stockwise ask "how is my portfolio doing?"
claude mcp add stockwise -- stockwise mcp     # Claude Code
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

## Live trading

See [brokers.md](brokers.md). Orders are previewed and need a typed `yes`.
Every `trade` command takes `--broker angel|zerodha|upstox|dhan|fyers`
(default `angel`, or `STOCKWISE_BROKER`); all but `angel` are experimental —
untested against a live account.

| Command | Description |
|---------|-------------|
| `trade setup` / `trade config [--keyring]` | How to connect / create the owner-only credentials file, move secrets into the OS keyring |
| `trade login` | Daily login (Zerodha, Upstox, Fyers) or paste a Dhan token |
| `trade holdings` / `positions` / `orders` | Live holdings (synced into `portfolio`, keeping your lots), open positions, order book |
| `trade buy <SYMBOL> <QTY> [--limit P] [--trigger T] [--dry-run] [--yes]` | Buy on NSE: MARKET, LIMIT, SL-M (`--trigger`) or SL (both) |
| `trade sell <SYMBOL> <QTY> [--limit P] [--trigger T] [--dry-run] [--yes]` | Sell on NSE, same order types |
| `trade limit <SYMBOL> <QTY> <PRICE> [--dry-run] [--yes]` | Limit buy on NSE |
| `trade gtt <buy\|sell> <SYMBOL> <QTY> --trigger T --limit P [--dry-run] [--yes]` | Single-leg GTT (not Upstox) |
| `trade gtt list [--notify] [--json]` / `trade gtt cancel <ID>` | List GTTs with status changes since the last look (triggered, cancelled, expired, gone; `--notify` sends them) / cancel one (asks for confirmation) |
| `trade sync [--since DATE] [--apply] [--notify] [--json]` | Today's executed delivery trades into the portfolio — buys become dated lots, sells come out FIFO — idempotent by broker trade id; previews unless `--apply`. Dhan also reads older fills with `--since`. See [brokers.md](brokers.md#syncing-trades-and-gtts-into-the-portfolio) |
| `trade selftest [--redact-amounts] [--json]` | Read-only check of the integration against your account (session, profile, holdings, positions, orders, trades, GTTs, instrument lookup, a dry-run order, response shapes); prints a redacted report for a [broker test report](brokers.md#help-verify-a-broker). Exits 1 if a check failed |

## Setup

| Command | Description |
|---------|-------------|
| `config [show] [--json]` | Effective settings and where each comes from (flag, env, config file, default), plus the config file's status |
| `config path` | Print the config file path (it may not exist yet) |
| `config init [--force]` | Write a commented template to the config file path; refuses to replace an existing file without `--force` |
| `notify [status]` / `notify test` | Show the configured notification channels (Telegram, Slack, Discord, email, desktop) / send a test message. See [notifications](bots.md#notifications) |
| `completions <bash\|zsh\|fish\|powershell\|elvish>` | Print a completion script, e.g. `stockwise completions zsh > ~/.zfunc/_stockwise` |
| `man [--dir <DIR>]` | Print the `stockwise.1` man page, or write it plus one `stockwise-<command>.1` page per command into `DIR`, e.g. `stockwise man --dir ~/.local/share/man/man1` |

## Scripting with `--json` and `--csv`

```bash
stockwise portfolio --json | jq '.totals'
stockwise portfolio --json | jq -r '.holdings | sort_by(.pnl_percent) | .[:3][] | "\(.symbol) \(.pnl_percent)%"'
stockwise portfolio --csv > portfolio.csv                 # open in a spreadsheet
stockwise history TCS -p 1y --csv > tcs.csv               # every daily bar
stockwise alert check --json | jq -r '.[] | select(.status == "triggered") | .symbol'
stockwise screen dividend --csv | column -s, -t
stockwise tax in --json | jq '.summary.total_tax'
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
- Fields ending in `_percent` are percentages (`12.5` = 12.5%). `dividend_yield`
  in `quote`, `compare` and `screen` is a fraction as Yahoo reports it (`0.012`
  = 1.2%). Prices are as Yahoo returns them; derived money values are rounded
  to 2 decimals, indicators to 4.
- Dates are `YYYY-MM-DD`; `history` bar times are RFC 3339 in UTC.
- CSV follows RFC 4180 quoting (fields with a comma, quote or newline are
  quoted, quotes doubled), starts with a header row, and ends lines with `\n`.
  CSV columns are the same names as the JSON row keys.
- The `--ai` section is never part of JSON or CSV output.
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
| `quote` | `[quote]` — Yahoo's quote fields, plus `dataSource` when another provider supplied the price, and `deliveryPercent`, `lowerCircuit`, `upperCircuit` when NSE did ([data-providers.md](data-providers.md#nse-and-bse)) | quote: `symbol, name, currency, price, change, change_percent, open, day_low, day_high, year_low, year_high, volume, avg_volume_3m, market_cap, pe_ttm, eps_ttm, dividend_yield, delivery_percent, lower_circuit, upper_circuit` (the last three empty unless NSE answered) |
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
| `compare` | `[{symbol, name, currency, price, change_percent, market_cap, pe_ttm, pe_forward, eps_ttm, price_to_book, dividend_yield, profit_margin, revenue_growth, debt_to_equity, beta, year_high, year_low, sma_50, analyst_rating}]` — the table's transpose | stock |
| `correlate` | `{period, interval, symbols: [..], matrix: [[..]]}` — `matrix[i][j]` pairs `symbols[i]` and `symbols[j]` | symbol: `symbol, <SYM1>, <SYM2>, ...` |
| `returns` | `{symbol, currency, price, returns: [{period, label, start_price, end_price, return_percent, annualized_percent, growth_of_100000}]}` | period |
| `movers` | `{gainers: [row], losers: [row]}`, row = `{side, rank, symbol, price, change_percent, volume, currency}` | stock, gainers then losers |
| `sectors` | `[{sector, symbol, price, change_percent, currency}]`, best first | sector |
| `screen` | `[{symbol, name, price, change_percent, pe, market_cap, dividend_yield, currency}]` | match |
| `dividends` | `{symbol, currency, price, pays_dividend, dividend_yield_percent, annual_dividend_per_share, income: [{invested, shares, annual_income, monthly_income}], drip: [{year, shares, growth_percent}]}` | — |
| `backtest` | `{symbol, currency, strategy, period, interval, bars, start, end, fill, capital, start_value, end_value, total_return_percent, cagr_percent, max_drawdown_percent, sharpe, sortino, exposure_percent, trade_count, win_rate_percent, avg_win_percent, avg_loss_percent, profit_factor, costs_paid, gross_return_percent, buy_hold_return_percent, alpha_percent, skipped_entries, costs, cost_model, rules, trades: [trade], equity: [{date, value}]}` — returns are after costs except `gross_return_percent`; `portfolio` and `walkforward` shapes are in [backtesting.md](backtesting.md#json-output). trade = `{symbol, entry_date, exit_date, qty, entry_price, exit_price, gross_pnl, costs, net_pnl, pnl_percent, bars_held, exit_reason}` | trade (every form) |
| `sip` | `{symbol, currency, period, interval, monthly_amount, months, total_invested, final_value, profit, total_return_percent, xirr_percent, total_units, avg_cost, current_price}` | — |
| `whatif` | `{symbol, currency, amount, date, buy_date, buy_price, shares, invested, current_price, value, pnl, pnl_percent, cagr_percent, days, years}` — `buy_date` is the trading day closest to `date` | — |
| `portfolio` | `{holdings: [holding], totals: {cost_basis, value, pnl, pnl_percent, currency}, unpriced: [symbol], unconverted: [symbol]}`, holding = `{symbol, shares, avg_cost, cost_basis, price, value, pnl, pnl_percent, weight_percent, xirr_percent, currency}` — a holding's money values are in its own currency; `totals` and `weight_percent` are in the base currency (`--base`); `unconverted` holdings had no exchange rate | holding |
| `watch` | `[{symbol, price, change, change_percent, volume, currency}]`, in watchlist order | symbol |
| `alert list` | `[{index, symbol, condition, target, created_at, period, description}]` — `index` is what `alert remove` takes; `condition` is the CLI name (`above`, `rsi-below`, ...), `period` the RSI or volume-average length (else null), `description` the condition in English words | alert |
| `alert check` | `[{index, symbol, condition, target, price, currency, status, period, description, value, reference}]` — `status` is `triggered`, `waiting`, `no_price` (price alert without a live price) or `no_data` (too little history, or no results date); `value` is what the condition compared (price, RSI, day change %, volume multiple, days to results; the close for crosses and breakouts) and `reference` the moving average, previous high/low or average volume | alert |
| `calendar` | `[{date, days_until, when, symbol, event, detail, source, estimate}]` — `when` is `today`, `tomorrow`, `this_week` (within 7 days) or `later`; `event` is `results`, `board_meeting`, `ex_dividend`, `dividend_paid`, `split`, `bonus`, `rights` or `other`; `source` is `yahoo` or `nse`; `estimate` marks Yahoo's unconfirmed earnings dates. Notes (e.g. NSE refused) go to stderr | event |
| `portfolio benchmark` | `{benchmark: {symbol, name, currency, converted_to_base}, base_currency, period, start, end, trading_days, portfolio_return, benchmark_return, excess_return, portfolio_annualized, benchmark_annualized, metrics: {observations, alpha, beta, tracking_error, information_ratio, correlation}, holdings: [holding], sectors: [{sector, holdings, avg_weight, end_weight, return_pct, contribution, active_contribution}], flagged: [{symbol, reason}], series: [{date, portfolio, benchmark}]}`, holding = `{symbol, sector, avg_weight, end_weight, return_pct, contribution, active_contribution, days_held, days_unpriced, flag}` — returns, weights, `alpha` and `tracking_error` are percentages, contributions are percentage points; annualized returns only for periods over about 13 months; `series` is growth of 100; figures are null when nothing was held | holding |
| `tax export` | `{country, tax_year, period_start, period_end, currency, disclaimer, sales: [sale], summary, other_currency: [symbol], virtual_digital_assets: [symbol], notes: [text]}` (`virtual_digital_assets` for India only) — India: sale = `{section, term, isin, scrip, symbol, quantity, sale_date, sale_price, sale_value, purchase_date, purchase_price, cost, holding_days, gain, grandfathered, purchase_date_estimated}`, summary = `{stcg_111a_sale_value, stcg_111a_cost, stcg_gains, stcg_losses, ltcg_112a_sale_value, ltcg_112a_cost, ltcg_gains, ltcg_losses, st_loss_set_off_against_ltcg, taxable_stcg, ltcg_after_set_off, ltcg_exemption, taxable_ltcg, stcg_tax, ltcg_tax, total_tax, st_loss_carried_forward, lt_loss_carried_forward}`; US: sale = `{part, description, symbol, quantity, date_acquired, date_sold, proceeds, cost_basis, gain_or_loss, date_acquired_estimated}`, summary = `{short_term_proceeds, short_term_cost, short_term_gain, long_term_proceeds, long_term_cost, long_term_gain, net_gain, taxable_short_term, taxable_long_term, loss_deductible_this_year, loss_carryover, short_term_tax, long_term_tax, total_tax}` | — (the CSV is the schedule itself, see [Tax export](#tax-export)) |
| `goal sip` | `{target, years, annual_return_pct, step_up_pct, savings, savings_future_value, monthly_sip, final_monthly_sip, total_invested, gains, lump_sum_today, schedule: [{year, monthly_sip, invested, value}], goal, currency}` — `goal` is `sip`, `retire` or `education` | — |
| `goal retire` | `{monthly_expense_today, years_to_retire, years_in_retirement, inflation_pct, return_pre_pct, return_post_pct, monthly_expense_at_retirement, corpus, sip: plan, goal, currency}` — `plan` is the `goal sip` object without `goal`/`currency` | — |
| `goal education` | `{cost_today, years, inflation_pct, future_cost, sip: plan, goal, currency}` | — |
| `tax` | `{country, tax_year, currency, sales: [{symbol, qty, bought, bought_estimated, sold, gain, term}], holdings: [{symbol, cost, value, short_term_gain, long_term_gain, estimated_dates}], realized: estimate, if_sold_today: estimate, ltcg_exemption_left, unpriced: [symbol], other_currency: [symbol]}`, estimate = `{short_term_gains, short_term_losses, long_term_gains, long_term_losses, taxable_short_term, taxable_long_term, exemption_used, short_term_tax, long_term_tax, total_tax, loss_carried_forward}` — `term` is `short`, `long`, or null (UK); `ltcg_exemption_left` is India only | open holding |
| `sim history` | `{stats: {days, winning_days, day_win_rate_percent, total_pnl, avg_daily_pnl, best_day, worst_day, trades, winning_trades, losing_trades, trade_win_rate_percent, sharpe, max_win_streak, max_loss_streak}, confidence, sessions: [{date, market, capital, target_percent, settled, trades, pnl, pnl_percent, wins, losses}]}` — `confidence` is `high`, `moderate`, `low`, or null before 5 settled days | — |
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

## Results and corporate calendar

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
```

| Key | Overridden by | Built-in default |
|-----|---------------|------------------|
| `market` | `-m`, `STOCKWISE_MARKET` | `in` |
| `ai` | `--ai` / `--no-ai`, `STOCKWISE_AI` | `false` |
| `model` | `STOCKWISE_MODEL` | `qwen3:14b` |
| `ollama_host` | `OLLAMA_HOST` | `http://localhost:11434` |
| `redis_url` | `STOCKWISE_REDIS_URL` | `redis://127.0.0.1:6379/` |
| `home` | `STOCKWISE_HOME` | platform data directory (see `doctor`) |
| `lang` | `--lang`, `STOCKWISE_LANG` | your locale, else `en` |
| `quote_currency` | `--quote-currency`, `STOCKWISE_QUOTE_CURRENCY` | `usd` |
| `portfolio` | `--portfolio`, `STOCKWISE_PORTFOLIO` | `default` |
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
the key. `config`, `doctor` and `completions` still run, so you can inspect it
or replace it with `stockwise config init --force`.

API keys (`OPENAI_API_KEY`, the data provider's key, ...) and
`STOCKWISE_API_TOKEN` are read from the environment only. Notification tokens
and passwords may go in `[notify]`; if they do, keep the file private
(`chmod 600`) — `config show` and `doctor` warn when other users can read it.

`stockwise serve` answers with the same shapes over HTTP — see
[server.md](server.md).
