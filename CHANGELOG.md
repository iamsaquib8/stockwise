# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.2] - 2026-11-22

### Added
- `verdict --horizon short` (also `portfolio review` and `picks --horizon short`) sets the
  invalidation at the higher of close − 1.5×ATR and the 20-day low, reviews in 10 sessions and
  needs a rising trend and volume before a BUY. Small caps (below ₹5,000 crore or $2 billion)
  get AVOID on a pledge above 10%, thin trading, repeated price-band hits, auditor red flags or
  exchange surveillance, and at most half the usual position size.
- `verdict`, `deep` and `stockwise SYMBOL` start with a rule-based action (BUY, WATCH or AVOID;
  ADD, HOLD, TRIM or EXIT for a holding; NO CALL below 60% data coverage) with its reasons,
  confidence, invalidation price and review date. New `portfolio review` lists each holding's
  action (EXIT, TRIM, ADD, HOLD) with size and tax notes; `portfolio`, `portfolio checkup` and
  `picks` gain an Action column, `--json` an `action` object, and MCP the `stock_action` and
  `portfolio_review` tools.
- `portfolio xray` adds up Indian business groups (Tata, Adani, Reliance,
  Bajaj, Aditya Birla, public sector and more), directly and through index
  funds, and `portfolio analyze` notes when one promoter group is over 20% of
  the portfolio.
- `portfolio add` and `portfolio sell` take `--charges auto` for NSE/BSE
  delivery trades: STT, stamp duty, the exchange charge in force that day,
  SEBI fee, GST and the DP charge on a sell are estimated, shown and kept on
  the lot or sale (brokerage is left out; give it with `--charges N`).
- After the close the worker sends one digest instead of a message per
  notice: the day's P&L, top contributors and detractors with the news or
  filing behind each move (or flagged unexplained), monitor signals, alerts
  that triggered, the LTCG, results and filing notices, and dates coming up.
  `stockwise digest` prints it; `--send` sends it now.
- `portfolio analyze` lists ELSS units still in their three-year lock-in
  under "tax if sold", with the date each purchase becomes redeemable.
- The worker's filing notices now cover BSE (`.BO`) holdings through BSE
  announcements and US holdings through SEC EDGAR (8-K, 10-Q, 10-K and more).
- `stockwise hot` shows what's hot: your holdings and watchlist ranked by
  heat (excess move in σ, volume surge, fresh news or filings, a date within
  three days, scaled by portfolio weight), each with its move and a one-line
  why; then the market's biggest moves, unusual volume, gaps, 52-week highs
  and lows, most-mentioned stocks, sectors and FII/DII flows, NSE bulk and
  block deals. `--cap small` and `--json`.
- `today`, `hot`, the daily digest and the Telegram `/review` show each
  stock's rule-based action (EXIT and TRIM first in `/review`). The
  after-close message also carries a target-drift notice when a target is out
  of its band, and an advance-tax reminder a week before an instalment is due
  (India).
- Allotment odds now use application counts (lots available ÷
  applications), not 1 ÷ subscription, and each category shows the
  expected value of a minimum application net of the blocked money's cost.
- `stockwise ipo`, `ipo SYMBOL` and the MCP `ipo_board` / `ipo_analyze` tools fall
  back to BSE's public-issue pages for anything NSE does not supply: the open and
  forthcoming list, price band, lot, dates, issue size, fresh/OFS split and the
  category subscription multiples. Values are merged field by field (NSE's value
  is kept when it has one), and a new `meta` object names the sources, the
  fields BSE filled (`field_sources`), what no source had (`missing`), what
  failed (`warnings`) and the key-field `coverage`; text output ends with the
  same footer. A field neither exchange supplies stays empty and lowers the
  verdict's data coverage, so an incomplete issue gets NO CALL instead of a
  guess. BSE is asked only when NSE leaves a gap.
- Listing-day plan and anchor lock-in dates; listed issues hand over to the
  stock action engine.
- MCP: `ipo_analyze` and `ipo_board` carry the `verdict` object; new prompt
  `should_i_apply_ipo`.
- The IPO-market regime is the recency-weighted median listing-day return
  of the last 180 days (8 listings minimum), not today's price of listings
  of different ages.
- `stockwise ipo review` (track record per verdict: listing day, three
  months, against NIFTY 50) and `stockwise ipo calibrate` (refit the model).
- The verdict rests on a listing-day model fitted by robust regression on
  past NSE mainboard issues (expected gain and the probability of listing
  below the issue price from its residuals), valuation at the upper band
  against listed peers from the RHP figures you save
  (`--rhp-template`), and hard gates (P/E over 1.5× peers, a fund-exit OFS
  over 75%, losses without growth, QIB or total under 1x at close,
  negative GMP, SME red flags).
- `stockwise ipo` gives every Indian IPO a rule-based verdict: APPLY, APPLY –
  SELL ON LISTING, WAIT (with the QIB multiple by 3 PM IST on the closing day
  that would make it APPLY), AVOID or NO CALL, with up to three reasons,
  confidence, invalidation and review date (IPO methodology `ipo-1`,
  `docs/methodology.md`). It replaces the 6-pillar IPO score.
- MCP: new tools `whats_hot`, `today`, `news_feed`, `monitor_status` and
  `digest`, each with an output schema.
- `stockwise mcp` for Indian investors: new tools `bonds_yields`,
  `bonds_list` and `bonds_calc` (G-secs and T-bills), `commodity` (gold,
  silver and Sovereign Gold Bonds in rupees), `fx_history` (USD/INR),
  `returns`, `correlate`, `compare` and `portfolio_wrap` (calendar or Indian
  financial year), prompts `mf_review` and `advance_tax_check`, ticker
  completion that puts NSE and BSE listings first, and instructions that
  cover rupee units, market hours and Indian capital-gains rules.
- `stockwise mcp` upgrade: the main tools (`quote`, `history`, `verdict`,
  `picks`, the portfolio tools, `rebalance`, `tax_estimate`,
  `market_conditions`) publish an `outputSchema` their structured results
  follow; every tool carries read-only, destructive, idempotent and
  open-world hints; symbol arguments complete from your holdings, watchlist
  and ticker search. New tools `picks_review` and `verdicts` (up to 25
  tickers, as `quote` now takes), new resources `stockwise://picks/latest`,
  `stockwise://track-record`, `stockwise://methodology` and
  `stockwise://quote/{symbol}`, subscriptions that notify when the
  portfolio, watchlist, alerts or profile change, six new prompts (daily
  briefing, should I buy, exit check, small-cap short-term scan, tax-aware
  rebalance, weekly review), per-call timeouts (`STOCKWISE_MCP_TIMEOUT`) and
  instructions for the recommendations policy. `picks` lists asked for by an
  assistant are logged to the track record too.
- The background worker now watches every holding with no setup: a day drop
  of 5% or 2σ, a new 52-week low, a fall past a threshold from cost or from
  the peak, a gap at the open, results, ex-dividend, record date or split
  tomorrow, promoter pledge or shareholding changes, analyst rating changes
  and insider trades. Thresholds live in `[monitor]`; `stockwise monitor`
  shows them and runs a pass.
- `portfolio pme` and `portfolio wrap` compare Indian portfolios with a NIFTY 50
  total-return series (NIFTYBEES' dividend-adjusted closes; BANKBEES for NIFTY
  Bank), so your dividends count on both sides; without it they fall back to
  the price index with dividends left out.
- `notify` supports ntfy and Pushover, gives every notice a priority, and
  `STOCKWISE_NOTIFY_ROUTES` (e.g. `desktop=high,telegram=low`) sends each
  channel only the priorities it should get.
- `rebalance` is tax- and cost-aware: each sell lists its lots (losing lots
  first, then the lowest tax, lots turning long-term within 45 days last, where
  the ledger allows choosing), its STCG/LTCG and the tax it adds, and each trade
  its STT, stamp duty, exchange fees, DP charge, brokerage (`--brokerage`) and FX
  spread (`--fx-spread`), with a `--min-trade` floor. New `--raise AMOUNT` sells
  only, at the lowest tax; `--inflow N --with-sells` adds new cash to a full
  rebalance; `--cash-buffer` keeps cash aside; asset-class targets (`equity`,
  `debt`, `gold`, `international`, `cash`) use the `xray` look-through.
- `[profile] residence = "in"` (or `"us"`, `"uk"`) sets the country whose
  holding periods and tax rules `portfolio analyze` applies; without it the
  analysis says which residence it assumed from the market.
- `stockwise portfolio target drift` (no percentage) shows current against
  target for every target key with the 5/25 band (5 points or 25% of the target,
  whichever is smaller), as text or `--json`; `drift_notice_today` gives the
  scheduled drift notice for the background notifier.
- `stockwise today` shows what changed in your portfolio today: each
  holding's day move and P&L with why it moved (the headline or exchange
  filing behind it, "with the market", or "unexplained" for an unusual move
  with no news), the market backdrop, dates in the next three days, and one
  news feed for holdings and the watchlist ranked by weight × importance.
- `portfolio` shows total return: unrealized, realized (this financial year
  and all time) and dividends, plus the 1-day, 1-week, 1-month and
  year-to-date change of current holdings; XIRR is hidden for lots with an
  unknown purchase date.
- `portfolio analyze` shows the portfolio's value-weighted P/E, P/B and
  dividend yield, with how much of the value each covers and which holdings
  have no trailing P/E.
- `worker install` runs the background worker at login as a per-user launchd
  agent (macOS) or systemd user service (Linux), restarting it after a
  failure; `--dry-run` prints the unit and commands. `worker uninstall` and
  `worker service-status` remove and check it.
- AI commands retry a cloud provider's rate limit or overload (HTTP 429, 503,
  529) up to three times, waiting as `Retry-After` asks (at most 30 s)
  ([#114](https://github.com/iamsaquib8/stockwise/issues/114)).
- `daemon longterm` sends its findings through your notification channels,
  inside the day's digest ([#118](https://github.com/iamsaquib8/stockwise/issues/118)).
- `STOCKWISE_QUIET_HOURS` (e.g. `22:00-07:00`) holds every notice below
  high priority until the quiet hours end and then sends them as one message
  ([#121](https://github.com/iamsaquib8/stockwise/issues/121)).
- A global `--verbose` (`-v`) flag and the `STOCKWISE_LOG` variable print network
  diagnostics on stderr: each request's source, host and path, status and time,
  and with `STOCKWISE_LOG=debug` cache hits. Query strings (keys, tokens) are never
  printed ([#179](https://github.com/iamsaquib8/stockwise/issues/179)).
- `quote DXY -m fx` (and `USDX`, `DX-Y.NYB`) shows the US dollar index, and
  `markets --fx` adds the AED, SGD, JPY, AUD, CAD and CHF rupee pairs
  ([#18](https://github.com/iamsaquib8/stockwise/issues/18)).
- `stockwise init` walks a new user from install to a first portfolio: it
  saves the default market, imports a broker file (`kite`, `groww`, `upstox`,
  `indmoney`, `cas`), shows the Claude / Cursor MCP config and prints three
  next commands. Every step has a flag (`--market`, `--import SOURCE:FILE`,
  `--no-mcp`) and nothing is prompted without a TTY. The no-argument screen
  starts with "New here?" while the portfolio is empty
  ([#192](https://github.com/iamsaquib8/stockwise/issues/192)).
- `tax reconcile FILE` compares StockWise's realised gains with a broker's tax P&L (Zerodha
  Console, Groww, Upstox; CSV or `.xlsx`) sale by sale, names the likely reason for each
  difference (cost, sale value, buy date, quantity, grandfathering, charges, term), lists rows
  it could not read, and totals STCG/LTCG on both sides, with `--json` and `--csv`. Closes [#203](https://github.com/iamsaquib8/stockwise/issues/203).
- `tax income` lists dividends and SGB interest for a financial year with gross, estimated TDS
  (10% of a company's dividends once they pass ₹10,000 in the year, ₹5,000 up to FY 2024-25)
  and net in paise-exact amounts, split by advance-tax period; `tax export in --schedule os`
  writes it for ITR Schedule OS (naming anything left out), and `tax advance --slab 30` adds it
  to the instalments. Closes [#246](https://github.com/iamsaquib8/stockwise/issues/246).
- `tax in`, `tax export in`, `tax advance` and `harvest` now use the Section 112A grandfathered
  cost for long-term lots bought on or before 31 Jan 2018: the higher of the actual cost and the
  lower of the 31 Jan 2018 price and the sale price. The export adds fair market value and
  actual cost columns. Closes [#247](https://github.com/iamsaquib8/stockwise/issues/247).
- `portfolio sync <broker>` compares your lots with the broker's holdings
  through a read-only session and reports the drift per symbol with the likely
  fix (an unapplied split or demerger, a missing buy, a missing sale). It writes
  nothing unless you pass `--apply`, which adds only the missing buys. The worker
  runs the same read-only comparison every day after the close for each broker
  with a saved session, and `portfolio sync` shows the latest reports. Closes
  [#254](https://github.com/iamsaquib8/stockwise/issues/254).
- `tax foreign` and `tax export in --schedule fa|form67` give Schedule FA (calendar-year
  initial, peak and closing values of foreign shares at each day's rupee rate, flagging any
  missing rate) and Form 67 foreign tax credit inputs at the `--withholding` rate from your
  statement; `tax in` and the audit pack apply Sec. 50AA to debt funds bought from 1 Apr 2023
  and market-linked debentures, Sec. 112 to other funds and SGBs, and Sec. 115BBH (30% flat,
  no set-off, 1% TDS) to crypto, same-day sales included. Closes [#264](https://github.com/iamsaquib8/stockwise/issues/264).
- `stockwise telegram` is a two-way Telegram bot: `/today`, `/hot`, `/review`
  and `/digest` from your chat, and a digest every trading day at
  `STOCKWISE_DIGEST_TIME` ([#265](https://github.com/iamsaquib8/stockwise/issues/265)).
- Kite, Upstox, Dhan and Fyers instrument lists (15–30 MB each) are now
  downloaded once per IST day and kept in `cache/instruments/` in the data
  directory, with the parsed list held in memory for the rest of the process.
  A second order on the same day makes no download; a new day, or a damaged
  cache file, downloads the list again. Closes
  [#31](https://github.com/iamsaquib8/stockwise/issues/31).
- `import --account NAME` names the demat account a file belongs to, so two
  accounts at one broker (yours and a family member's) keep separate lots and
  sales, and re-importing either file changes nothing. Closes
  [#315](https://github.com/iamsaquib8/stockwise/issues/315).
- `quote`, `verdict`, `deep`, `longterm` and `picks` end with one line naming the sources,
  the as-of time and the missing fields, and their `--json` has a `meta` object (`sources`,
  `as_of`, `missing`, `warnings`, `coverage`); `longterm` gains `--json`
  ([#338](https://github.com/iamsaquib8/stockwise/issues/338)).
- `verdict`, `deep` and the bare `stockwise SYMBOL` fill fundamentals Yahoo leaves empty: US stocks
  from SEC EDGAR, then Finnhub/FMP when keyed; Indian stocks from NSE and BSE. Each filled field is
  named in `meta.field_sources` ([#339](https://github.com/iamsaquib8/stockwise/issues/339), part of [#361](https://github.com/iamsaquib8/stockwise/issues/361)).
- When Yahoo Finance fails, FX pairs and the large coins keep working
  through two keyless sources. Frankfurter serves the ECB's daily reference
  rates for `-m fx` quotes and history and for portfolio and statement
  currency conversion, labelled "ECB reference rate, daily". CoinGecko serves
  crypto prices and up to 365 days of daily history for `history`, `risk`,
  `correlate` and `backtest`; `COINGECKO_API_KEY` takes an optional free Demo
  key. The output footer and `--json` `meta.sources` name the source, a symbol
  neither source carries keeps Yahoo's error, and
  `STOCKWISE_KEYLESS_BACKUPS=off` turns both off
  ([#341](https://github.com/iamsaquib8/stockwise/issues/341)).
- `stockwise goal` is a horizon-aware, post-tax planner. Without `--return`,
  `goal sip` and `goal education` match the return to the horizon (capital
  preservation to 24 months, conservative hybrid to 60, long-term growth
  beyond), accept `--months`, `--horizon 18mo` and `--by YYYY-MM`, estimate tax
  and exit load per instalment at redemption, show the SIP that leaves the
  target after tax (`--net`), `--inflation`, P10 / median / P90 outcome bands
  and a ±2% return sensitivity, and warn when an equity return is forced on a
  short goal. `--from-portfolio` and `--holdings` link existing savings to the
  portfolio; `goal add|list|show|remove` keep goals in `goals.json`;
  `goal retire` reports the withdrawal rate and survival after a 30% first-day
  fall; new `goal swp`, `goal emergency` and `goal prepay`, and MCP tools
  `goal_swp`, `goal_prepay` and `goal_list`
  ([#356](https://github.com/iamsaquib8/stockwise/issues/356)).
- `--cap all|mega|large|mid|small|micro` on `movers`, `screen` and `heatmap`,
  served from fresh `cache.db` snapshots when the worker has them.
- `--universe nifty500|midcap150|smallcap250`: one NSE request prices the whole
  index, so `movers` and `heatmap` work when Yahoo Finance is unreachable and
  every index scan feeds `market_snapshots`
  ([#357](https://github.com/iamsaquib8/stockwise/issues/357)).
- Provider router: `FINNHUB_API_KEY` (quotes, profile, analyst trend, earnings
  surprises) and `FMP_API_KEY` (quotes, quarterly statements, screener) back
  Yahoo up per capability, respecting each free tier's window and merging
  fields with `meta.sources`; `results` and US scans keep working when Yahoo
  fails, `doctor` and `config show` list the keys, and the BSE provider reads
  announcements, results filings and group members
  ([#357](https://github.com/iamsaquib8/stockwise/issues/357)).
- `sectors <NAME> [--cap all|mega|large|mid|small|micro]` lists an Indian sector's member stocks with their cap tier (`cap_tier` in `--json`/`--csv`).
- Market-cap tier badge (`mega`, `large`, `mid`, `small`, `micro`) in `quote`,
  a bare `stockwise SYMBOL`, `deep`, `analyze`, `compare`, `movers`, `screen`
  and `portfolio`, with a `cap_tier` field in `--json` / `--csv`. India follows
  the SEBI/AMFI ranks (NIFTY 100 / MIDCAP 150 / SMALLCAP 250 / outside NIFTY
  500) from saved index members, with rupee cut-offs offline; every other
  market converts the cap to dollars (London pence and yen included). One
  classifier now serves `cache.db` snapshots and `portfolio xray`.
- `stockwise hot` lists cross-market movers among Indian and US large caps,
  with relative volume paced for the time of day while the session is open
  ([#358](https://github.com/iamsaquib8/stockwise/issues/358)).
- `stockwise hot` tracks theme baskets (AI, semiconductors, defence, battery
  and EV, minerals and metals) for India and the US, with each basket's
  equal-weight day move and its best and worst member
  ([#359](https://github.com/iamsaquib8/stockwise/issues/359)).
- `stockwise results AAPL -m us` reads SEC EDGAR (keyless `companyfacts` XBRL
  from 10-K and 10-Q filings) for 12 or more quarters and the fiscal years
  behind them, instead of Yahoo's last four: revenue, operating income, net
  income, diluted EPS, operating cash flow and capex. Q4 is derived as the
  fiscal year minus Q1 to Q3, cash-flow quarters from year-to-date
  differences, restated facts take the latest filing, and September or
  52/53-week year ends work. Yahoo fills the fields EDGAR lacks and the
  source is listed; periods are stored in `cache.db` tagged `edgar`. Requests
  carry a generic `stockwise` User-Agent (no email) and stay under 5 a second.
- `stockwise results SYMBOL` collates the last 8 or more quarters of reported
  results side by side: revenue, operating profit and margin, other income,
  interest, depreciation, pre-tax profit, tax rate, net profit and diluted EPS,
  with YoY and QoQ growth, margin change in basis points, a trailing-twelve-month
  column, and EPS against consensus with the 1-day and 5-day move versus the
  index. It asks Yahoo's `fundamentals-timeseries` with explicit period bounds
  (`quoteSummary` carries four quarters). Banks add net interest income and
  gross/net NPA when the NSE filing has them. `--annual` shows fiscal years with
  CFO/PAT, free cash flow, ROE, ROCE and 3/5/10-year CAGR. `results --holdings`
  and `results --universe nifty50` print an earnings-season scoreboard with
  aggregate YoY growth and beat/in-line/miss counts. All forms take `--json` and
  `--csv`, every fetched period is stored in the `financial_results` table of
  `cache.db`, and the MCP server gains a `quarterly_results` tool
  ([#361](https://github.com/iamsaquib8/stockwise/issues/361)).
- `stockwise hot -m in` shows the NSE F&O ban list and open-interest
  build-up (long or short build-up, short covering, long unwinding)
  ([#362](https://github.com/iamsaquib8/stockwise/issues/362)).
- `stockwise profile` introduces investor profile modes (`passive`, `active:low|med|high`, `ultra`) and personal guardrails across research, goals, rebalancing, technical analysis, and tax:
  - `stockwise profile set <MODE[:RISK]> [--tax-slab PCT] [--tax-regime new|old] [--horizon-years N] [--max-stock-weight PCT] [--max-smallcap-weight PCT] [--risk-per-trade PCT]` writes `[profile]` to `config.toml`, and `stockwise profile [show] [--json]` displays effective guardrails and their origin (`flag > env > config file > default`).
  - Global `--profile <MODE[:RISK]>` (and `STOCKWISE_PROFILE`) overrides the active profile for any command.
  - `passive` mode automatically enables educational explanations in `verdict`, `score`, `technical`, and `tax`, highlights P10–P50 high-confidence bands and auto-enables glide path for long-horizon goals, defaults rebalance drift to 5.0%, and flags small-cap or single-stock concentration exceeding configured guardrails.
  - `ultra` mode introduces a dense Pro Terminal View for bare symbol lookup (`stockwise <SYM>`) featuring microstructure, verdict, accounting forensics, 8Q earnings delta, technicals & classic pivot levels, and F&O buildup, plus 1R/holding portfolio heat tracking.
  - `stockwise stoploss` adds position sizing with `--capital` and `--risk-pct` (defaulting to the profile's `risk_per_trade`), computing share quantities and position values against the risk budget.
  - Exposed MCP tool `profile` and resource `stockwise://profile` for AI terminal assistants
  ([#374](https://github.com/iamsaquib8/stockwise/issues/374), [#22](https://github.com/iamsaquib8/stockwise/issues/22)).
- `stockwise import cas FILE.pdf [--password P]` converts a password-protected
  CAS PDF through `casparser` when it is on `PATH` (password from
  `--password`, `STOCKWISE_CAS_PASSWORD` or a hidden prompt), `import cas -`
  reads casparser output from standard input, and a missing `casparser` prints
  a one-line install-and-run command instead of a UTF-8 error
  ([#375](https://github.com/iamsaquib8/stockwise/issues/375)).
- `stockwise mf bleed` compares held Regular-plan funds with their Direct
  twin: TER gap from AMFI's disclosure, yearly rupee cost, 10-year compounded
  cost, and the units that can switch tax-free inside the Sec. 112A exemption.
  `portfolio` points to it when a Regular plan is held. New MCP tool `mf_bleed`
  ([#375](https://github.com/iamsaquib8/stockwise/issues/375)).
- `stockwise portfolio sell SYMBOL QTY [PRICE] --preview` (alias `--dry-run`)
  shows the FIFO lots a sale would take, the short- and long-term capital-gains
  tax with the unused Sec. 112A exemption, grandfathered cost and 4% cess, a
  countdown for lots within 45 days of turning long-term (with the tax saved by
  waiting) and a mutual-fund exit-load check, without changing `portfolio.json`.
  `sellpreview::ltcg_countdown` is a pure function the background worker can
  call for post-close notifications. New MCP tool `portfolio_sell_preview`
  ([#375](https://github.com/iamsaquib8/stockwise/issues/375)).
- `stockwise portfolio checkup` lists findings in six lenses (concentration,
  forensic flags, valuation, downside, fees and tax, goals), ranked by the
  money at stake, each with the command to inspect it; `--offline` uses only
  the cache, and MCP has `portfolio_checkup`. `stockwise portfolio wrap`
  reviews a year (`--year 2026`, `--fy 2026-27`) against the benchmark, and
  `--redact` hides every amount so the card can be shared
  ([#376](https://github.com/iamsaquib8/stockwise/issues/376)).
- The background worker's post-close sync now also notifies about new
  non-routine NSE exchange filings of held `.NS` stocks (date, title and
  category, plus `stockwise events SYMBOL`), once per filing; the `eod` task
  result reports a `filings` count
  ([#377](https://github.com/iamsaquib8/stockwise/issues/377)).
- The background worker's post-close sync now sends notices through your
  notification channels: when a held lot is about to turn long-term (7-day
  heads-up) or has turned, and when a held stock reports a new quarter. Each
  is sent once; `stockwise tasks add eod` queues the same sync for the worker
  ([#377](https://github.com/iamsaquib8/stockwise/issues/377)).
- Zero-setup persistent cache and background worker. Without Redis, provider
  responses are kept in a SQLite (WAL) `cache.db` in the data directory, so
  the next command reuses fresh quotes and NAVs. The same file now holds
  `market_snapshots` (cap tier, sector, P/E, RVOL, ROE, yield),
  `ohlcv_bars`, `nav_history`, `financial_results` and a `tasks` queue.
  New: `stockwise cache status|clear|warm|snapshots`, `stockwise worker
  start|stop|status|run` (quotes while the market is open, queued tasks,
  alert checks, a post-close sync of snapshots, fund NAVs and wealth
  history), and `stockwise tasks [list|show|cancel|add]`. `serve` exposes
  `GET /api/events` (SSE), `tui` refreshes on live price changes, and
  `/metrics` and `doctor` report `cache.db`. `STOCKWISE_CACHE_DB=off`
  disables the file
  ([#377](https://github.com/iamsaquib8/stockwise/issues/377)).
- `stockwise ask` knows your investor profile, saves conversations
  (`-c`, `--resume ID`, `--sessions`), holds one in the terminal (`-i`), runs
  prepared tasks (`--playbook checkup|tax-season|earnings`), and with `--auto`
  may change your workspace, undoable with `stockwise undo`
  ([#378](https://github.com/iamsaquib8/stockwise/issues/378)).
- `stockwise connect` adds StockWise's MCP server to the AI apps on your
  computer in one step: Claude Desktop and Code, Cursor, Windsurf, VS Code,
  Cline, Gemini CLI, Google Antigravity, OpenAI Codex, Zed, LM Studio and
  opencode (Goose and Continue get lines to paste). It backs up each config,
  changes only the `stockwise` entry, installs the agent skill where the app
  reads skills, and `--remove` takes it out
  ([#378](https://github.com/iamsaquib8/stockwise/issues/378)).
- `stockwise mcp --allow-writes` lets an assistant change your watchlist,
  alerts, goals, targets, notes and profile (with a few-question profile
  onboarding), and `--allow-portfolio` record lots; `stockwise undo` reverses
  any change. Nine more read tools, among them `portfolio_analyze`,
  `market_conditions`, `picks` and `fit_check`, which says whether a stock
  fits your own rules
  ([#378](https://github.com/iamsaquib8/stockwise/issues/378)).
- The `stockwise trade` order preview (and `--dry-run`) now lists estimated
  charges for NSE equity: brokerage and the DP charge on delivery sells at
  each broker's published rates, STT, the exchange transaction charge in
  force on the day, SEBI fee, stamp duty and GST, as delivery and as
  intraday side by side, plus an estimated slippage line from the bid-ask
  spread for MARKET and SL-M orders. STT, SEBI fee, stamp duty and GST use
  the backtest's own rates. `--dry-run --json` and `--dry-run --csv` print
  the preview and the charges as data. Part of
  [#382](https://github.com/iamsaquib8/stockwise/issues/382).
- `stockwise use` shows or switches the default market and the active
  portfolio (`stockwise use us`, `stockwise use long-term`), and
  `--bind` ties a portfolio to a market. Without `-m`, ISINs, exchange
  suffixes and tickers that only one market lists resolve to the right
  market offline (`stockwise NVDA` with India as the default quotes NVIDIA);
  tickers listed in several markets keep the current one and print a hint
  ([#388](https://github.com/iamsaquib8/stockwise/issues/388)).
- `backtest` strategy files can size positions (`fixed-fraction`, or
  `atr-risk` to lose a set share of capital on an ATR move), trade
  fractional shares, sell short with a borrow fee, trail stops by a percent
  or by ATR, earn interest on idle cash and pay capital-gains tax on
  realised gains. Every option is optional and old files give the same
  results. Reports add the Probabilistic and Deflated Sharpe ratios, with
  the `[grid]` combinations as trials
  ([#394](https://github.com/iamsaquib8/stockwise/issues/394)).
- `stockwise backup export [PATH.swbak]` writes portfolios (default and
  named), goals, watchlist, alerts, notes, the undo log, practice history and
  `config.toml` to one passphrase-encrypted file (Argon2id key derivation,
  XChaCha20-Poly1305, authenticated header with format version and KDF
  costs). The archive inside is deterministic. Caches, lock files, rotating
  backups, broker session files and notification secrets are left out.
  `stockwise backup restore PATH.swbak [--dry-run]` checks the whole file
  first (a wrong passphrase or a changed byte stops it cleanly), lists the
  files it would add or replace, keeps `<file>.bak` copies and writes each
  file atomically. The passphrase comes from a hidden prompt, `--passphrase-fd N`
  or `STOCKWISE_BACKUP_PASSPHRASE`, never from an argument.
  `stockwise portfolio export --format beancount|hledger [-o FILE]` writes the
  portfolio as a plain-text double-entry journal: commodity and account
  declarations, lot purchases at cost, sales at their matched cost with
  `Income:CapitalGains:ShortTerm` / `LongTerm` legs, and dividends, in date
  order; symbols become valid commodity names (`MF:120503` → `MF_120503`)
  ([#397](https://github.com/iamsaquib8/stockwise/issues/397)).
- `stockwise update` (alias `self-update`) updates a binary installed with
  install.sh or from a release archive: it downloads the new archive and its
  `.sha256`, refuses to continue if the checksum is missing or wrong, and swaps
  the binary atomically. Homebrew, Scoop, winget, Nix, Cargo, npm, pipx, uv,
  pip, `.deb`/`.rpm` and Docker installs are left alone and the command prints
  their upgrade command. `--check` only reports the versions. `stockwise doctor`
  says when a newer release is out, checking at most once a week; no other
  command checks for updates
  ([#398](https://github.com/iamsaquib8/stockwise/issues/398)).
- `tax export in --audit-pack DIR` writes one workpaper CSV per rupee sale (lot, dates, cost,
  31 Jan 2018 FMV where it applies, gain to the paisa and the rule applied), a `summary.csv`
  and the Schedule CG CSV, all or nothing. Part of [#399](https://github.com/iamsaquib8/stockwise/issues/399).
- `income` (and `portfolio income` / `portfolio dividends`) adds a 12-month
  trailing and projected passive income calendar with terminal monthly volume
  charts, Yield-on-Cost (YoC) per holding and per lot, TDS/withholding tax
  estimates (10% India Section 194/194K, 25% US WHT), Sovereign Gold Bond (2.5%
  p.a.) half-yearly coupon schedules, factual Free Cash Flow and payout ratio
  sustainability checks, upcoming ex-dividend radar (`income upcoming`), and
  the `income_calendar` MCP tool
  ([#403](https://github.com/iamsaquib8/stockwise/issues/403)).
- `portfolio pme` (Index Twin / Public Market Equivalent) mirrors your exact historical cash
  flows into a passive benchmark (SPY, VOO, VWCE, NIFTY50, NIFTYBEES), computing Kaplan-Schoar
  PME, Direct Alpha, and net currency value added vs the index ([#404](https://github.com/iamsaquib8/stockwise/issues/404)).
- `portfolio actions` detects more corporate actions and shows how each lot
  would change before `--apply` records it: reverse splits, US spin-offs with
  the cost split from each company's Form 8937 (GE → GEHC and GEV, 3M →
  Solventum, Danaher → Veralto, Merck → Organon, IBM → Kyndryl), rights issues
  (Reliance 2020, Bharti Airtel 2021; reported, not recorded), and
  `portfolio return-of-capital` (`roc`) cuts the cost of the lots held on the
  ex-date. Every `--apply` is journalled: `--journal` lists the entries and
  `--undo` takes back the newest. Closes
  [#405](https://github.com/iamsaquib8/stockwise/issues/405).
- `statusbar` prints one compact line for tmux, waybar, sketchybar or starship
  (`▲ ₹+12.3k (+0.8%) · RELIANCE +1.2% · TCS −0.4%`), plus `--format json`. It
  answers from a local cache, fetches at most once per `--refresh` seconds
  within 3 s, shows the cached line marked stale when offline and stays quiet
  while markets are closed ([#408](https://github.com/iamsaquib8/stockwise/issues/408)).
- Market scans cover whole markets. India scans every stock listed on BSE
  that traded ₹10 lakh or more on the last session (about 1,900, from BSE's
  end-of-day bhavcopy; NIFTY 500 when it is unavailable), the US scans
  the S&P 1500, and London, Germany, Japan, Hong Kong, Singapore, Canada and
  Australia scan their main index (FTSE 350, DAX and MDAX, Nikkei 225, Hang
  Seng, STI, S&P/TSX Composite, S&P/ASX 200) instead of 15 stocks each. `--universe` takes `all` (every BSE stock), `bse` (its main
  board) or an NSE index, whose members come from NSE, else niftyindices.com,
  else a built-in list; `picks`, `matrix`, `surprise` and `longterm` take it
  too. Scans reuse quotes kept in `cache.db` for 15 minutes, and
  `stockwise cache clear` takes a target (`http`, `quotes`, `snapshots`,
  `bars`, `results`, `members`, `bhavcopy` or `all`). Outside Indian market
  hours, `movers` and `heatmap` price every stock from the bhavcopy
  ([#431](https://github.com/iamsaquib8/stockwise/pull/431)).
- `stockwise conditions` shows where a market stands, as readings: the main
  index against its 200- and 50-day averages and its highs, India VIX or VIX
  against five years, the share of scanned stocks above their averages, and
  aggregate P/E and earnings yield against the 10-year government yield.
  Breadth and valuation are saved each session (also by the daily
  `daemon longterm` run) and ranked against the sessions saved before. `--json` prints the report. Readings of the data: not a forecast and not a
  signal to invest or to wait
  ([#435](https://github.com/iamsaquib8/stockwise/pull/435)).
- `stockwise portfolio analyze` (also `portfolio dash`, and key `p` in
  `stockwise tui`) shows the portfolio by market (with each market's own
  currency total), currency, cap tier and sector; unrealised gains split into
  short- and long-term with the next lots to turn long-term and the estimated
  tax if everything were sold today; volatility, largest fall, beta and
  concentration; and neutral observations, each with its figures.
  `portfolio add --currency INR` converts a price paid in another currency at
  that day's exchange rate
  ([#437](https://github.com/iamsaquib8/stockwise/pull/437)).
- `portfolio lots SYMBOL --lot N` can now change a lot's quantity (`--qty`) and
  price (`--price`) or delete it (`--delete`), and `--undo-sale N` puts a
  recorded sale back as the exact lot it came from (date, price, split record,
  account). Each portfolio save keeps the previous five versions
  (`portfolio.json.1` to `.5`); `portfolio restore` lists them and
  `portfolio restore N` brings one back. `portfolio remove` asks before
  dropping a holding with more than one lot (`--yes` skips the question)
  ([#94](https://github.com/iamsaquib8/stockwise/issues/94)).
- `portfolio demerger PARENT CHILD --ratio A:B --cost-split PCT [--date D]`
  records any demerger or spin-off: the parent's lots keep their quantity,
  purchase dates and the rest of the cost, and the new company's lots get the
  same dates. `portfolio rename OLD NEW` moves a holding's lots, sales and
  dividends to a new ticker. Both are journalled and safe to repeat. Closes
  [#96](https://github.com/iamsaquib8/stockwise/issues/96).
- `tax export us` finds wash sales: a loss with the same shares bought within
  30 days gets code `W` and the disallowed loss in Form 8949 columns (f) and
  (g), and `harvest -m us` flags loss candidates bought in the last 30 days
  ([#97](https://github.com/iamsaquib8/stockwise/issues/97)).
- `tax advance` shows the next instalment (15 Dec, 15 Mar, ...) with the amount left to pay and
  the Sec. 234C interest if it is missed, applies the capital-gains and dividend proviso only
  when their tax is paid by 31 March, and `--paid DATE:AMOUNT --record` keeps payments per
  financial year for later runs and for the reminder notifiers can send seven days ahead. Part
  of [#259](https://github.com/iamsaquib8/stockwise/issues/259).
- `doctor` also checks notification channels, the background worker and its
  login service, which AI apps have the MCP server, and the investor profile and
  tax residence; every problem comes with a fix, and `doctor --json` prints the
  checks for scripts.
- `quote`, `verdict` and `chart` for Indian stocks: when Yahoo has no data for `X.NS` (or `X.BO`) the other
  listing on Yahoo stands in; `quote` shows NSE's sector P/E; NSE surveillance (ASM/GSM), suspension and
  delisting flags appear as warnings; banks and NBFCs no longer count debt/equity as missing data.
- `picks --horizon short` ranks for weeks to months: trend (3-month momentum
  per unit of volatility, 50-day average, rising volume) carries 45% and risk
  20%. `--min-mcap` / `--min-adv` set the floors.
- `picks review` shows the track record. Every `picks` list is logged to
  `signal_log.json`, and each pick is measured from the day it was first listed
  against NIFTY 50 or the S&P 500 total return, with hit rate and mean and
  median excess return.
- `portfolio lots SYMBOL --json` prints the holding's stored lots, sales and
  dividends for scripts and bug reports
  ([#223](https://github.com/iamsaquib8/stockwise/issues/223)).
- `chart`, `risk`, `verdict` and backtests keep working when Yahoo's price history fails: Indian stocks
  and NSE indices fall back to NSE (or BSE) history adjusted for splits and bonuses, then to bars cached
  in `cache.db`, with a warning that says how old they are.
- `profile set --residence in|us|uk` saves the country of tax residence, `profile
  show` prints it, and `doctor` points to the flag when it is not set.

### Changed
- `verdict` and the other stock views now give rule-based actions by default instead of
  neutral readings only. The rules are published in the methodology, AI only explains the
  computed action, and the disclaimer now says these are educational signals, StockWise is not
  a SEBI-registered adviser and losses are possible.
- `stockwise --help` shows the core commands, including `today`, `hot`, `portfolio
  review`, `picks review` and `tax reconcile`; `stockwise --help-all` lists every
  command ([#150](https://github.com/iamsaquib8/stockwise/issues/150)).
- `--json`: every key ending in `_pct` now also appears as its `_percent` twin
  (`weight_pct` and `weight_percent`), so scripts can use the documented
  `_percent` suffix everywhere; the `_pct` names are deprecated and go in 1.0
  ([#223](https://github.com/iamsaquib8/stockwise/issues/223)).
- `tax`, `tax export in`, `tax advance` and `harvest` now tell gold, silver, international,
  liquid and debt ETFs by their category in NSE's ETF list, with a longer built-in table and
  name patterns as the offline fallback, so a new non-equity ETF is no longer taxed as equity.
  Closes [#318](https://github.com/iamsaquib8/stockwise/issues/318).
- `daemon intraday` keeps one market-data client for the life of the loop
  instead of rebuilding it after every third failed tick; the client already
  renews its own session, so a failure now only changes the retry delay
  ([#342](https://github.com/iamsaquib8/stockwise/issues/342)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `goal sip` prints "1 year" instead of "1 years", no longer defaults to 12%
  on short goals, and the MCP `goal_sip` / `goal_education` tools need `years`,
  `months` or `by` instead of assuming one year at 12%
  ([#356](https://github.com/iamsaquib8/stockwise/issues/356)).
- Redis is now opt-in: without `STOCKWISE_REDIS_URL` or `redis_url`, StockWise
  no longer probes `127.0.0.1:6379` and uses the SQLite `cache.db` instead.
  Chart fetches also keep their complete OHLCV bars in `cache.db`, and
  `worker start|stop` works on Windows
  ([#377](https://github.com/iamsaquib8/stockwise/issues/377)).
- `picks` ranks stocks against each other: each of four pillars (value,
  quality, growth, trend) is a percentile rank among the stocks scanned,
  replacing eight fixed yes/no rules. The table shows every pillar and how
  much of the score had data; `--cap` ranks one cap tier within itself, and
  `--top N`, `--json` and `--csv` are new
  ([#431](https://github.com/iamsaquib8/stockwise/pull/431)).
- `risk`, `deep`, `longterm`, `chart`, `timeframes` and backtests measure returns and volatility on Yahoo's
  dividend-adjusted closes; NSE, BSE and Alpha Vantage history is adjusted for splits and bonuses.
  `picks --json` is now an object with `picks` and `meta`.
- Docs: new guides for rule-based actions (`portfolio review`, `verdict`),
  `hot`, `today` and `digest`, and `picks` with `picks review`; an accurate
  source map in `docs/architecture.md`; and `docs/troubleshooting.md` with the
  `doctor` line and fix for each common failure
  ([#201](https://github.com/iamsaquib8/stockwise/issues/201),
  [#303](https://github.com/iamsaquib8/stockwise/issues/303)).
- `serve`: `docs/server.md` lists every endpoint, including `/api/research`
  and `/api/portfolios`, and every query parameter each one takes
  ([#111](https://github.com/iamsaquib8/stockwise/issues/111)).
- Harden repository `.gitignore` against local databases (`cache.db*`), user portfolios, model weights, and editor caches, and add universal AI developer context files (`CLAUDE.md`, `AGENTS.md`, `CODEX.md`, `GEMINI.md`, `.cursorrules`, `.windsurfrules`, `.clinerules`).
- With `--json`, errors print as `{"error":{"message":…,"exit_code":…}}` on stderr
  instead of coloured text, so scripts can read them
  ([#223](https://github.com/iamsaquib8/stockwise/issues/223)).
- `README.md` is organised around four jobs: review your holdings
  (`portfolio review`), see what's hot (`hot`), find candidates (`picks`) and
  file your tax (`tax`), with real example output and every install channel
  ([#191](https://github.com/iamsaquib8/stockwise/issues/191)).

### Fixed
- `portfolio add --currency INR` keeps the rate you paid on the lot, so a US
  share bought with rupees is costed at the rupees paid; `portfolio add` and
  `portfolio sell` take `--charges`, and realized P&L is net of them.
- `alert`: `--on-close` judges moving-average crosses and breakouts on
  completed sessions only; crosses missed while nothing ran are caught up
  once; price levels are checked only on fresh quotes while their market is
  open; `--cross` notifies only when the price crosses the level; adding an
  identical alert is refused; and each alert has a stable id, so editing the
  list never re-sends or loses a trigger.
- `portfolio checkup` pairs correlations on shared dates, scales drawdown by
  the value it covers and says how much, and counts index funds by their
  constituents; mutual funds get a category or "Fund (no look-through)"
  sector instead of all sharing one.
- `portfolio actions --apply` demergers: splits before the record date count in
  the entitlement, shares sold after it get their new shares and a split cost,
  new shares are held only from the record date, a broker snapshot's shares
  are not counted twice, and a tradebook re-import keeps the demerger.
- `portfolio actions --apply` records Indian demergers (Reliance → JIOFIN,
  ITC → ITCHOTELS, Tata Motors → TMCV): each lot held on the record date gets
  a lot of the new company with the same purchase date and the cost share
  from the company's filing; the parent keeps the rest (Sec. 49(2C)/(2D)).
  Until then, `portfolio`, `analyze`, `checkup`, `income`, `xray` and `wrap`
  value a copy with it applied and say "demerger pending".
- `earnings` shows each holding's and watchlist stock's next results date,
  soonest first; `surprise` shows the latest quarter's actual against estimated
  EPS and the surprise %; `insider` lists insider trades from NSE PIT disclosures
  (India) and SEC Form 4 filings (US), with Yahoo Finance as a labelled fallback.
- `harvest` LTCG step-up rows give the exact share count whose gain fits the
  ₹1.25 lakh exemption left (oldest lots first) instead of the whole holding,
  label the saving "13% incl. cess", take losses brought forward
  (`--carry-st`, `--carry-lt`), and remind Indian investors to buy back on the
  next trading day. US loss rows name their lots and the command that records
  them: new `portfolio sell --lots N,N` sells specific lots of US and UK shares,
  so the ledger books the loss the broker reports.
- `heatmap` no longer panics when a quote's change is not a number; stocks
  without a change are listed last with "—" instead of as 0%.
- `portfolio` costs foreign holdings, sales, dividends and XIRR at each
  purchase or sale date's exchange rate (ECB reference rates, cached in
  `cache.db`) instead of today's, adds an FX effect column, and keeps closed
  positions in other currencies in the portfolio XIRR.
- `portfolio income` pays SGB interest on the issue price without TDS, applies
  dividend TDS only above ₹10,000 per company, and includes the current month;
  goals compound at the effective monthly rate and link each holding to one
  goal; `divcal` and `pcorr` weight by value and list what they left out.
- The worker's "lot turns long-term" notices find their lot by purchase date
  and quantity, so a FIFO sale of an older lot no longer loses the notice,
  and they now cover every market you hold, not just the worker's.
- Money now adds up exactly in the tax report (realized, unrealized and
  intraday totals, net of charges), per-lot short- and long-term gains in
  `portfolio analyze`, the PME value added and `sim` status, settle and
  session totals: amounts are summed in integer minor units (paise, cents)
  and converted to decimals only for display and JSON.
- `movers` says when its prices are from: the local time, the session date of
  the latest trade and whether the market is open. `dashboard` adds the
  portfolio's day change in value and percent.
- `news` searches by company name in the stock's own locale, keeps only the
  last `--days`, adds exchange and SEC filings, shows each item's link and
  local time, counts the sources that actually answered, and has `--json`.
- `portfolio`, `portfolio analyze`, `checkup`, `income`, `xray`, `pme` and
  `wrap` notice a split or bonus Yahoo reports but your lots have not taken
  yet, say "split pending: run `portfolio actions --apply`", and value the
  adjusted share count meanwhile, so a 2:1 split no longer shows a fake 50% loss.
- `portfolio pme` leaves holdings without a price out on both sides and says
  how many, never lets the Index Twin sell more than it holds, compares with a
  total-return index when one exists (else leaves dividends out on both
  sides), and flags cash flows dated before the index history.
- `rebalance` plans now balance their cash: whole-share sells pay for buys
  (plus any new cash), leftovers go to the holdings furthest below target and
  buys are trimmed by the same priority when cash runs short; the plan shows the
  net cash it frees or needs and the cash left. Projected weights come from the
  rounded orders instead of repeating the target, `--inflow` splits new cash the
  way that really minimizes the squared deviation from target, holdings without
  a price are left out with a warning instead of failing the plan, and the
  profile's `max_stock_weight` is a hard limit that no longer flags ETFs.
- `portfolio target` warns about keys that match no holding, sector or asset
  class instead of dropping them silently, carves a symbol target out of its
  sector target instead of counting both, and accepts symbols not held yet
  (`rebalance` gives them a buy line). `target remove` resolves the symbol like
  `target set`, `target show` prints the real default drift band, and `checkup`
  measures drift against the same normalized targets as `rebalance`.
- Fixed a second round of review findings. Action methodology is now 2: ADD
  checks the combined direct plus fund look-through weight against the cap,
  and the size hint leaves room for indirect exposure. An unreadable notes file
  gives holdings NO CALL instead of acting as if there were no notes. MCP
  `stock_action` reads the caller's workspace portfolio, and `deep` sends the
  cloud AI only the public rule-based action. Also: realized tax buckets net
  eligible charges, per-lot term gains use purchase-date FX, foreign holdings
  without base-currency history get no beta, checkup tax rules follow
  residence, quoted profile keys are replaced, effective bets with partial
  index data are labelled an upper bound, PME twin keeps unsold value,
  `sim settle` keeps trades the daemon already closed, `sim status` leaves
  unquoted trades out of the total, the volume baseline needs full session
  coverage, long-term scores treat a zero P/E as unusable and count a P/E pair
  as growth data, picks uses one earnings calendar per scan, and MCP write
  tools report honest idempotence.
- Fixed IPO verdict and BSE fallback edge cases (missing category books no
  longer default to 0x, `--board sme` filters before ongoing enrichment, numeric
  BSE scrip codes resolve to `.BO`, `WAIT` thresholds round up, NSE anchor share
  counts survive BSE split merges, 180-day regime edge is included, and RHP
  template zeros are commented out), `GBp` market-cap and own-P/E scaling in the
  action engine, look-through key matching in `portfolio review`, alert
  cross/delivery rollback, off-market insider classification, walk-forward
  carried short trades, advance-tax `relief_lost` status, demerger child
  reconciliation, rebalance cap clamping, `monitor_status` offline access,
  session refusal locking, `-100%` day-change guards, action invalidation and
  filings-coverage gating, minor-unit intraday/insider math, EDGAR/pulse/worker
  hardening, and documentation and skill accuracy.
- `portfolio analyze` measures risk on base-currency closes without cutting
  every holding to the shortest history, uses a blended IN/US benchmark for
  mixed portfolios, and `portfolio benchmark` alpha is over a risk-free rate
  with at least 60 observations.
- `rotation` and `sectorcmp` match each sector to its own quote by symbol, so a
  quote Yahoo drops no longer shifts numbers onto the wrong sector, and show
  relative strength against NIFTY 50 / S&P 500 over 1, 3 and 6 months ("—" when
  missing) instead of overweight/underweight labels from the day's change.
- `sentiment` takes `-m in|us`: India reads India VIX (with its own, higher
  bands), NIFTY 50, NIFTY MIDCAP 50 and BANK NIFTY. Headline sentiment (here and
  in `events`) matches whole words, so "commissioned" no longer counts as "miss"
  or "windfall" as "fall", handles negation ("not a loss", "no growth") and uses
  one shared word list; corporate actions are no longer scored positive by default.
- `portfolio analyze` picks the tax term by where you live: for an Indian
  resident, foreign shares turn long-term at 24 months and debt funds bought
  from 1 April 2023 never do. "Tax if sold" lists foreign, other-rule and
  unpriced holdings in their own sections instead of dropping them.
- `portfolio wrap` converts each amount at its own date's exchange rate,
  measures alpha like for like (with dividends against a total-return index,
  without them against a price index), and shows no Modified Dietz return
  over a near-empty starting base.
- `portfolio xray` leaves out and lists holdings without a price or exchange
  rate, looks through NIFTYBEES, BANKBEES, JUNIORBEES, SPY/VOO/IVV and QQQ with
  shipped index weights instead of equal weights, matches gold and silver by
  exact symbol, and labels funds it can't classify as unknown.
- `tui`: each `s` sort order is distinct and labelled in the pane titles (file
  order, symbol, change %, value), file order comes back at once, holdings sort
  by value in the base currency, and the selection stays on the same symbol
  when a refresh reorders the rows
  ([#104](https://github.com/iamsaquib8/stockwise/issues/104)).
- `tui`: the "Refreshing…" message clears when a manual refresh finishes, so
  the key hints come back ([#105](https://github.com/iamsaquib8/stockwise/issues/105)).
- `serve`: `portfolio=default` shows `portfolio.json` even when another
  portfolio is active, the dashboard's picker starts on the active portfolio,
  and an unknown name is a 404
  ([#109](https://github.com/iamsaquib8/stockwise/issues/109)).
- `serve` returns JSON errors for a query value of the wrong type (400) and
  for a method other than GET (405) instead of plain text or an empty body
  ([#110](https://github.com/iamsaquib8/stockwise/issues/110)).
- Research, `options` (NSE option chains), `calendar`, `results` and `hot` reach
  nseindia.com through the one shared NSE session: one cookie handshake, one
  rate limit, and no more requests after NSE refuses. The session now loads its
  cookies from the option-chain page (NSE refuses scripts on its home page), and
  an endpoint that refuses (such as `quote-equity`) is left alone for 15 minutes
  without blocking the others ([#159](https://github.com/iamsaquib8/stockwise/issues/159)).
- `quote DOW -m us` and `quote DAX -m us` look up Dow Inc. and the Global X DAX
  ETF instead of the indices; `DJIA`, `^DJI` and `^GDAXI` still give the indices
  ([#162](https://github.com/iamsaquib8/stockwise/issues/162)).
- `-m crypto` accepts exchange-style pairs: `BTC/USD`, `eth_inr` and `BTC USD`
  resolve to `BTC-USD` / `ETH-INR` instead of asking Yahoo for
  `BTC/USD-USD`
  ([#163](https://github.com/iamsaquib8/stockwise/issues/163)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `events`: US news after 16:00 New York is assigned to the next session using
  the real close (20:00 or 21:00 UTC with daylight-saving time)
  ([#165](https://github.com/iamsaquib8/stockwise/issues/165)).
- `history` reports Yahoo's 404 for an unknown symbol as "No price history for
  <symbol>" with the market hint — `GET /api/history/{unknown}` now answers 404
  instead of 502 — and the same holds for the `history` command
  ([#167](https://github.com/iamsaquib8/stockwise/issues/167)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- Long-running `serve`, `tui`, `mcp` and `ask` sessions keep the in-memory
  cache bounded (500 entries, 64 MB, one-hour TTL) instead of holding every
  researched symbol for as long as the process lives; Redis still keeps the
  long-lived copies
  ([#168](https://github.com/iamsaquib8/stockwise/issues/168)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `insider` no longer prints an empty "Analyst Consensus" header, and `export
  portfolio` and `wealth` show the same empty-portfolio hint as other commands
  ([#213](https://github.com/iamsaquib8/stockwise/issues/213), [#224](https://github.com/iamsaquib8/stockwise/issues/224)).
- `deep` prints `Income on ₹1L` for Indian stocks and `Income on $10k` for US
  stocks instead of the literal `Income on {}1L`
  ([#213](https://github.com/iamsaquib8/stockwise/issues/213)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `news` and `sentiment` show the publisher name (e.g. `Liquide Blog`) instead
  of the raw `url="...">` attribute from Google News items, and no longer repeat
  the publisher at the end of the title
  ([#214](https://github.com/iamsaquib8/stockwise/issues/214)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `markets` prints index levels and VIX as plain points (`25,030.35`, `25.64`)
  without a currency sign, and money in tables groups digits — lakhs for INR
  (`₹1,25,000`, `₹1,20,092.03`) and thousands otherwise (`$104,250.50`) — in
  the `portfolio`, `tax` and X-Ray tables and price columns; JSON and CSV stay
  ungrouped
  ([#217](https://github.com/iamsaquib8/stockwise/issues/217)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `portfolio` draws one allocation bar per holding, heaviest first, so the
  chart still reads with `NO_COLOR` set or with more than seven holdings
  ([#218](https://github.com/iamsaquib8/stockwise/issues/218)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `rebalance` no longer prints `SELL 0 shares` for a position less than one
  share over target; it reports OK instead, and one-share orders read
  `1 share` ([#219](https://github.com/iamsaquib8/stockwise/issues/219)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `portfolio sell` now ends with `stockwise portfolio lots <SYMBOL>` so the
  hint can be copied and run, the empty states of `harvest`, `rebalance` and
  `tax in` point at `stockwise portfolio add ...`, and the `events` footer no
  longer says "Buy/Sell guidance"
  ([#224](https://github.com/iamsaquib8/stockwise/issues/224)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- After a merger such as HDFC into HDFC Bank, `portfolio actions --apply`
  keeps the whole shares and records the cash paid for the fractional share as
  a sale at the record date's close, with its own gain. The total cost is
  unchanged. Closes [#317](https://github.com/iamsaquib8/stockwise/issues/317).
- `serve` stops exporting a daemon's session gauges from `/metrics` once its
  saved date is no longer the current session, so yesterday's open positions no
  longer read as today's
  ([#342](https://github.com/iamsaquib8/stockwise/issues/342)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `search` falls back to the saved NSE/BSE symbol list when an exchange
  answers 200 with an unreadable page, not only when the request fails
  ([#342](https://github.com/iamsaquib8/stockwise/issues/342)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `funds` no longer falls back to a TER spreadsheet saved more than a week ago
  when AMFI is unreachable; an older copy is refused with a message naming the
  limit ([#342](https://github.com/iamsaquib8/stockwise/issues/342)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- Every save now uses its own temp file, so two writes in one process (two
  commands, or one racing the daemon) can no longer rename or delete each
  other's half-written file ([#342](https://github.com/iamsaquib8/stockwise/issues/342)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- `results` bank asset quality now reads NSE's real banking XBRL (`PercentageOfGrossNpa` and `PercentageOfNpa`, filed as fractions), falls back to the standalone filing when the consolidated one has 0.00 placeholders, and skips list rows without an XBRL file.
- `intraday` measures the OBV trend relative to the average's magnitude, so a
  stock whose OBV is negative no longer reports a falling series as
  accumulation (or a rising one as distribution) and no longer emits the
  wrong "OBV Divergence" signal
  ([#41](https://github.com/iamsaquib8/stockwise/issues/41)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
- London stocks get the right cap tier: Yahoo gives their market cap in
  pounds while quoting prices in pence, and the tier read the cap as pence
  (Vodafone came out "small")
  ([#437](https://github.com/iamsaquib8/stockwise/pull/437)).
- `bonds tbill` refuses a yield and term whose discount factor is zero or negative
  instead of printing an infinite or negative price ([#57](https://github.com/iamsaquib8/stockwise/issues/57)).
- `bonds gsec` accepts the common ways people write a G-sec name: `7.18% G-Sec
  2033`, `7.18 GSEC 2033` and `7.18% GOI 2033` ([#58](https://github.com/iamsaquib8/stockwise/issues/58)).
- `backtest walkforward`: a trade carried into a test window from its train
  window is counted from the window's start (entry at the previous close, exit
  costs only), and trade money scales with each window's capital, so the
  out-of-sample trade statistics no longer include in-sample gains
  ([#78](https://github.com/iamsaquib8/stockwise/issues/78)).
- `intraday`: "VWAP Reclaim" and "VWAP Rejection" now need a real cross of the session
  VWAP (the previous bar on the other side), not just a price near it
  ([#79](https://github.com/iamsaquib8/stockwise/issues/79)).
- `options` (US): time to expiry counts to 16:00 New York at 21:00 UTC in winter,
  not 20:00 UTC all year, so winter IV and greeks no longer lose an hour
  ([#81](https://github.com/iamsaquib8/stockwise/issues/81)).
- `portfolio actions --apply` and tradebook imports record an Indian bonus
  issue as a new lot at zero cost dated on the allotment date, as Indian tax
  treats it, instead of as a split; the original lots keep their cost, so FIFO
  sales and capital gains come out right. Bonuses are told from splits with
  NSE's filings ([#95](https://github.com/iamsaquib8/stockwise/issues/95)).
- `screen dividend`, `picks` and `longterm` use the corrected dividend yield;
  Yahoo's trailing yield is 0 for many NSE stocks.
- `verdict` and the action columns say an ETF (such as NIFTYBEES, which Yahoo
  labels as equity) is not a listed company, instead of a data-coverage NO CALL.
- `portfolio review` and action sizing say "about 1 share", not "about 1 shares",
  and `cache snapshots`, `tasks` and `fundamental` describe every flag in `--help`.
- `technical`, alerts and backtests read the RSI of a flat series as 50 (it
  was 100, "overbought"). MACD with fewer than 35 bars gives no reading instead
  of a fake signal line of zero.
- `sim`, `bot` and `daemon intraday`: paper P&L is now after brokerage, STT,
  exchange and stamp charges and slippage (set with `STOCKWISE_SIM_COSTS`,
  `STOCKWISE_SIM_BROKERAGE`, `STOCKWISE_SIM_SLIPPAGE_BPS`). Exit levels sit at
  least 1 × ATR away, each trade risks a fixed 0.5% until 30 closed paper
  trades allow measured half-Kelly, plans rank by score, oversold readings
  share one vote, MACD reads three months of bars, VWAP and volume pace come
  from today's 5-minute bars, and Sharpe counts every trading day.
- `longterm` leaves pillars without data out of the composite. A debt/equity
  above 250 now gets its own band, and a negative P/E counts as a loss. The
  assumed return comes from earnings yield, growth and the last year's price
  return instead of the score itself, and the Monte Carlo median now matches
  the 5-year projection. The finalists' fundamentals are fetched before
  scoring ([#302](https://github.com/iamsaquib8/stockwise/issues/302)).
- `forecast` and `longterm` no longer assume 30% or 25% volatility when it can't be measured: bands and
  simulations show "missing". `forecast`'s "Expected Change" is now "Trend-line projection".
  Failed fundamentals sources are reported instead of dropped; `STOCKWISE_SEC_USER_AGENT` sets the
  contact SEC EDGAR asks for when it refuses requests (HTTP 403).
- `picks review` counts a pick's days to the session of today's prices, so a list
  made on a Saturday from Friday's closes shows 0 days, not 1; an ETF's NO CALL
  says it is not a listed company instead of listing company fields as missing.
- `picks` no longer puts stocks with broken or one-off figures at the top. Size
  and liquidity floors apply (India ₹500 crore and ₹2 crore traded a day, US
  $1 billion and $5 million, lower for `--cap small` / `--cap micro`).
  Fundamentals and a year of prices are fetched for the leaders before the
  final ranking. Losses and negative book value rank last, P/E below 3 or ROE
  above 80% are listed apart to check, stocks with under 60% of their data stay
  out, and at most 3 per sector are listed. There is a new risk pillar
  (volatility, drawdown, beta). See `docs/methodology.md`.
- `today`, `hot` and the digest complete each holding's and watched stock's quote
  before deciding its action, so they show the same BUY/HOLD/TRIM/EXIT as
  `portfolio review` instead of NO CALL (missing ROE, D/E) for every row; the
  disclaimer now prints with those actions, and digest amounts group in lakhs
  for rupees.

### Performance
- `stockwise <SYMBOL>` uses one Yahoo client for the research snapshot and the
  quote, saving a handshake and a repeated quote request
  ([#156](https://github.com/iamsaquib8/stockwise/issues/156)).
- With `STOCKWISE_DATA_PROVIDER=exchange`, NSE and BSE quotes and searches run at
  the same time, and keyed-provider quotes run alongside Yahoo's, so mixed
  watchlists load in about the time of the slower source
  ([#160](https://github.com/iamsaquib8/stockwise/issues/160)).
- `markets` (and `/api/markets` and the TUI) fetches every section in one quote
  request, with the India 10-year yield fetched alongside it
  ([#161](https://github.com/iamsaquib8/stockwise/issues/161)).
- `longterm` draws each Monte Carlo path's 5-year endpoint in one step instead of
  looping over 1,260 trading days, so scoring a list of stocks no longer spends
  most of its time in the simulation; the median and range are unchanged
  ([#37](https://github.com/iamsaquib8/stockwise/issues/37)).
- When NSE or BSE refuses StockWise, the refusal is remembered for 15 minutes
  across runs, so each `quote` of an Indian stock no longer spends a request and
  about 0.1 s asking again first.

### Security
- `doctor` and `trade config` now mask broker client, app and login IDs in diagnostics, reducing accidental disclosure in bug reports ([#298](https://github.com/iamsaquib8/stockwise/issues/298)).

## [0.8.1] - 2026-11-07

Fixes the wrong numbers and import data loss listed under "Known issues" in 0.8.0, and replaces the BUY/SELL verdict with neutral pillar scores.

### Added
- `score` fetches only the statements it needs, and `verdict`, `insights` and the bare-symbol snapshot skip the news and filing sources, so they answer faster. Every NSE request now times out after 3 seconds instead of stalling the command ([#222](https://github.com/iamsaquib8/stockwise/issues/222)).

- `score` fetches only the statements it needs, and `verdict`, `insights` and the bare-symbol snapshot skip the news and filing sources, so they answer faster. Every NSE request now times out after 3 seconds instead of stalling the command ([#222](https://github.com/iamsaquib8/stockwise/issues/222)).

### Changed
- `verdict`, `deep`, `analyze` and `stockwise SYMBOL` no longer print a STRONG BUY / SELL label, an entry zone, a stop-loss, a target price or risk/reward. They show six pillar scores (Strong / Moderate / Weak), a 0-100 composite and a "not a recommendation" note. In `--json`, `action`, `entry_zone`, `stop_loss`, `target_price`, `upside_pct`, `risk_pct`, `risk_reward`, the time-horizon texts and each pillar's `grade` are gone, and each pillar has a `band`. The `decision`, `signal` and `buysell` aliases are removed ([#188](https://github.com/iamsaquib8/stockwise/issues/188)).
- `verdict`: a pillar with no data now shows "no data" and is left out of the composite instead of scoring 50 or 55, and crypto, FX and index symbols get only the technical pillar and no composite ([#211](https://github.com/iamsaquib8/stockwise/issues/211)).
- `verdict --explain` now lists the real weights (valuation 20, growth 25, balance sheet 15, smart money 15, analyst 10, technicals 15), taken from the scores themselves, and the data sources that failed are listed under the score ([#211](https://github.com/iamsaquib8/stockwise/issues/211)).
- `verdict`: the fixed "Confidence: 85%" is replaced by `data_coverage` (JSON) / "Data coverage" (text), the share of the score's inputs that had data ([#77](https://github.com/iamsaquib8/stockwise/issues/77)).
- `insights`: analyst cards say "mean rating x/5" instead of "Strong Buy", and the 52-week-low card no longer calls it a "value entry".
- The `verdict` tool in the MCP server and in `stockwise ask` is now called "6-pillar scorecard", returns only pillar scores, data coverage and the facts behind them, and its description says it is not a recommendation; the buy/sell action, entry zone and stop-loss are no longer sent to the model ([#112](https://github.com/iamsaquib8/stockwise/issues/112)).
- `ipo`: the board and `ipo SYMBOL` no longer print APPLY / AVOID / SUBSCRIBE labels or the family-PAN "Retail Split Strategy". They show demand labels (strong, moderate, mixed or weak demand, book building), the allotment odds by application tier and, with `--capital`, the odds at that amount. In `--json`, `listing_verdict`, `long_term_verdict`, `timing_guidance`, `capital_plan` and `golden_rules` are replaced by `listing_demand`, `long_term_profile`, `timeline`, `capital_odds` and `allotment_rules` ([#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- `picks` is now "Highest Scores in the Large-Cap Universe" and lists the factors that raised each score instead of "reasons" such as "Analyst buy" ([#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- `intraday` is a signal scanner: it no longer gives entries, stop-losses, targets or position sizes for your capital, and shows the strategies behind the strongest signals instead. The `CAPITAL` and `TARGET%` arguments are accepted and ignored ([#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- The web dashboard's Research tab shows the pillar scores and data coverage instead of the buy/sell label, entry zone and stop-loss.
- The crates.io and PyPI description and keywords now mention portfolio, tax and the MCP server.

- `verdict`, `deep`, `analyze` and `stockwise SYMBOL` no longer print a STRONG BUY / SELL label, an entry zone, a stop-loss, a target price or risk/reward. They show six pillar scores (Strong / Moderate / Weak), a 0-100 composite and a "not a recommendation" note. In `--json`, `action`, `entry_zone`, `stop_loss`, `target_price`, `upside_pct`, `risk_pct`, `risk_reward`, the time-horizon texts and each pillar's `grade` are gone, and each pillar has a `band`. The `decision`, `signal` and `buysell` aliases are removed ([#188](https://github.com/iamsaquib8/stockwise/issues/188)).
- `verdict`: a pillar with no data now shows "no data" and is left out of the composite instead of scoring 50 or 55, and crypto, FX and index symbols get only the technical pillar and no composite ([#211](https://github.com/iamsaquib8/stockwise/issues/211)).
- `verdict --explain` now lists the real weights (valuation 20, growth 25, balance sheet 15, smart money 15, analyst 10, technicals 15), taken from the scores themselves, and the data sources that failed are listed under the score ([#211](https://github.com/iamsaquib8/stockwise/issues/211)).
- `verdict`: the fixed "Confidence: 85%" is replaced by `data_coverage` (JSON) / "Data coverage" (text), the share of the score's inputs that had data ([#77](https://github.com/iamsaquib8/stockwise/issues/77)).
- `insights`: analyst cards say "mean rating x/5" instead of "Strong Buy", and the 52-week-low card no longer calls it a "value entry".
- The `verdict` tool in the MCP server and in `stockwise ask` is now called "6-pillar scorecard", returns only pillar scores, data coverage and the facts behind them, and its description says it is not a recommendation; the buy/sell action, entry zone and stop-loss are no longer sent to the model ([#112](https://github.com/iamsaquib8/stockwise/issues/112)).
- `ipo`: the board and `ipo SYMBOL` no longer print APPLY / AVOID / SUBSCRIBE labels or the family-PAN "Retail Split Strategy". They show demand labels (strong, moderate, mixed or weak demand, book building), the allotment odds by application tier and, with `--capital`, the odds at that amount. In `--json`, `listing_verdict`, `long_term_verdict`, `timing_guidance`, `capital_plan` and `golden_rules` are replaced by `listing_demand`, `long_term_profile`, `timeline`, `capital_odds` and `allotment_rules` ([#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- `picks` is now "Highest Scores in the Large-Cap Universe" and lists the factors that raised each score instead of "reasons" such as "Analyst buy" ([#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- `intraday` is a signal scanner: it no longer gives entries, stop-losses, targets or position sizes for your capital, and shows the strategies behind the strongest signals instead. The `CAPITAL` and `TARGET%` arguments are accepted and ignored ([#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- The web dashboard's Research tab shows the pillar scores and data coverage instead of the buy/sell label, entry zone and stop-loss.
- The crates.io and PyPI description and keywords now mention portfolio, tax and the MCP server.
- `longterm` and `daemon longterm` describe scores as data, not calls: analyst lines give the mean rating and
  mean price estimate, risk tiers are `Lower` / `Moderate` / `Higher`, a pillar with no data shows a dash, and growth,
  SIP and simulation are labelled as an illustration ([#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- `analyze`, `insider`, `deep` and `compare`: the analysts' rating is shown as "mean rating 1.8/5 (1 = most positive)" instead of Strong Buy / Buy / Hold / Sell, and the price target line is now "Analyst Mean Price Estimate".
  `compare --json` and `--csv` give `analyst_mean_rating` (a number) instead of `analyst_rating` (a word). `verdict` words the same figure as an analyst mean price estimate above or below the current price.
- `analyze` and the research insight cards: the analyst lines say "mean price estimate" and give the mean rating as a number, instead of "target upside" and the Buy / Hold word.
- `ipo`: the press section counts positive and negative third-party headlines instead of "Subscribe / Avoid" mentions, and headlines that tell readers what to do are no longer repeated. In `--json`, `street_buzz.subscribe_mentions` and `avoid_mentions` are now `positive_headlines` and `negative_headlines`.
- `daemon intraday` and `sim`: paper positions are shown and sent in notifications as "long" / "short" with "level 1", "level 2" and "exit level" instead of BUY / SELL, ENTRY, T1, T2 and SL. Existing `daemon_state.json` and simulator history files still load.
- `technical` (and the `technical` MCP and `ask` tool): the RSI signal now reads "RSI above 70 (overbought zone)" or "RSI below 30 (oversold zone)" instead of "Consider selling" / "Consider buying" / "Strong buy signal". It describes the reading and no longer gives a call.
- The docs site is organised around goals: a five-minute quickstart, a "What do
  you want to do?" page and eight task-based guides (import trades, Indian
  capital-gains tax, quick lookups, rebalancing, Claude and Cursor over MCP,
  alerts and the daemon, backtests and practice, scripting). Each command
  reference section links to its guide.

### Fixed
- `import` of an `.xlsx` workbook with several sheets: only one sheet is read (the first whose header fits the importer, in tab order), so a mutual-fund or summary tab is no longer imported as bogus holdings. The sheets used and skipped are listed, and `--sheet NAME` picks another ([#101](https://github.com/iamsaquib8/stockwise/issues/101)).
- `import`: a tradebook with Mindtree trades (now recorded as LTIM) no longer stops with a 404 at the split check. If Yahoo has no price history for a symbol, the check is skipped for it, a note says so, and the rest of the file is imported ([#316](https://github.com/iamsaquib8/stockwise/issues/316)).
- `portfolio xray`: Indian stocks are now called large cap from ₹1,00,000 crore and mid cap from ₹33,000 crore (AMFI's list of December 2024), not ₹20,000 and ₹5,000 crore, so a ₹25,000 crore company shows as small cap and the market-cap split is no longer tilted to large caps ([#100](https://github.com/iamsaquib8/stockwise/issues/100)).
- `import groww` and `import upstox`: a holdings file whose ISIN column holds text such as `₹12,34,567` no longer crashes the import; the cell is treated as not an ISIN ([#102](https://github.com/iamsaquib8/stockwise/issues/102)).
- `import cas`: schemes you have fully redeemed are no longer skipped. When the statement covers every purchase, their redemptions are recorded as realized sales for `tax in`, `tax export in` and `tax advance`, and no holding is left. A redeemed scheme with units bought before the statement is listed as skipped, with the reason ([#90](https://github.com/iamsaquib8/stockwise/issues/90)).
- `import`: a second tradebook for the same broker is merged with the first instead of replacing its lots and sales, so yearly Zerodha Console files can be imported in any order and re-importing a file changes nothing ([#83](https://github.com/iamsaquib8/stockwise/issues/83)).
- `import` and `trade sync`: a buy and sell of the same share on one day are netted as an intraday sale before any older lot is touched, and `tax in`, `tax export in` and `tax advance` report intraday results separately instead of as 111A/112A gains from the oldest lot ([#84](https://github.com/iamsaquib8/stockwise/issues/84)).
- `import` and `trade holdings`: a holdings file from a second broker adds to the shares of the first instead of overwriting them. Lots and sales now carry the demat account (`zerodha`, `groww`, ...), FIFO, imports and syncs work per account, `portfolio lots` shows it, and old portfolio files load unchanged ([#85](https://github.com/iamsaquib8/stockwise/issues/85)).
- `import` and `portfolio add`: dated HDFC and Mindtree trades from before their mergers are now recorded as HDFCBANK (42 for 25) and LTIM (73 for 100) shares at the same total cost and date, instead of the old quantity at the old price ([#86](https://github.com/iamsaquib8/stockwise/issues/86)).
- `tax in`, `tax export in`, `tax advance` and `harvest`: gold, silver, liquid, debt and international ETFs (such as `GOLDBEES`, `LIQUIDBEES`, `MON100`) are no longer taxed as listed shares under Sec. 111A/112A; `tax in` lists them under "taxed under other rules" and `harvest` no longer offers them for the ₹1.25 lakh exemption ([#87](https://github.com/iamsaquib8/stockwise/issues/87)).
- `portfolio`, `import`, `trade sync`, `trade holdings` and `wealth`: commands that change your portfolio or wealth history at the same time no longer overwrite each other's changes. Each takes a lock, reloads the file and applies only its own change; a second command waits up to 30 seconds, then says so ([#93](https://github.com/iamsaquib8/stockwise/issues/93)).
- `deep`: no longer panics (`index out of bounds`) on NSE stocks when the latest bar has a missing close ([#306](https://github.com/iamsaquib8/stockwise/issues/306)).
- Yahoo fetch errors no longer include the request URL, so the session crumb cannot appear in error text.
- `portfolio xray` treats Indian ETFs (such as `NIFTYBEES`, `GOLDBEES`, `LIQUIDBEES`) as funds, also when Yahoo returns no holdings for them, instead of flagging them as one stock with too much weight. Gold, silver and liquid ETFs count as commodity and cash ([#220](https://github.com/iamsaquib8/stockwise/issues/220)).
- `longterm`: the SIP calculator now grows each monthly instalment only for the time it stays invested (₹15,000 a month at 12% gives ₹12.37 L after 5 years, not ₹15.86 L), the "SIP for" target reads `$1M` for US stocks, and the "Suggested SIP Allocation" block is gone ([#67](https://github.com/iamsaquib8/stockwise/issues/67), [#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- Yahoo Finance: commands no longer open the Yahoo session at start-up, so `history`, `backtest` and a provider-backed `quote` still work when Yahoo refuses the session token, and commands that never touch Yahoo send no request at all ([#154](https://github.com/iamsaquib8/stockwise/issues/154)).
- `quote`, `verdict`, `deep`, `score` and `insights`: a symbol Yahoo does not know, such as the BSE code `500325`, now gives a "not found" error with exit code 1 instead of an empty card or a verdict on a company with no price ([#206](https://github.com/iamsaquib8/stockwise/issues/206)).
- `score`, `analyze`, `verdict` and `deep`: companies that file statements in US dollars but trade in rupees, such as Infosys, now have their statement amounts converted at the live USD/INR rate before ratios and revenue are shown, so INFY's Altman Z drops from 426 to about 8 and its revenue reads ₹47.33K Cr, not ₹504 Cr ([#207](https://github.com/iamsaquib8/stockwise/issues/207)).
- `quote`, `compare`, `deep`, `dividends` and `divcal`: dividend yield no longer reads 0 for stocks such as RELIANCE, or 0.05% for INFY, when Yahoo's trailing figure is missing or computed from a dollar dividend; `dividends INFY` now shows 4.4% ([#208](https://github.com/iamsaquib8/stockwise/issues/208)).
- `peers`, `compare`, `deep` and `analyze`: sector, profit margin, debt/equity, revenue growth, beta and analyst ratings are now read from Yahoo's company summary, so `peers TCS` lists IT peers and `compare` no longer shows rows of N/A. Rows and sections with no data at all are left out ([#209](https://github.com/iamsaquib8/stockwise/issues/209)).
- `deep` and `analyze`: banks and lenders no longer show an Altman Z "Distress Zone" next to a strong score; they show "n/a for banks and lenders", and `score` says the same. The doubled brackets in the zone label are gone ([#212](https://github.com/iamsaquib8/stockwise/issues/212)).
- `forecast`: the default history is now 6 months instead of one, so `forecast TCS` prints a projection instead of "Not enough price data". The error now says how many days it found, and the `SYMBOL` argument has a help line ([#215](https://github.com/iamsaquib8/stockwise/issues/215)).
- `watch add`, `portfolio add` and `alert add` now refuse a ticker Yahoo does not know (for example `RELIANC`) and name the likely one, instead of saving it silently; `--force` saves it anyway, and funds, SGBs and offline use are not blocked. `technical`, `history`, `risk` and the other chart commands say "No price history for ZZQQXX.NS" instead of "HTTP 404" ([#216](https://github.com/iamsaquib8/stockwise/issues/216)).
- `portfolio` and `portfolio --json` now work offline: they list every holding with shares, average cost and cost basis, show prices as N/A (`price: null` and `unpriced` in JSON) and say "Offline: prices unavailable" instead of failing. `tax`, `harvest`, `rebalance` and `xray` still need a connection ([#221](https://github.com/iamsaquib8/stockwise/issues/221)).

- `import` of an `.xlsx` workbook with several sheets: only one sheet is read (the first whose header fits the importer, in tab order), so a mutual-fund or summary tab is no longer imported as bogus holdings. The sheets used and skipped are listed, and `--sheet NAME` picks another ([#101](https://github.com/iamsaquib8/stockwise/issues/101)).
- `import`: a tradebook with Mindtree trades (now recorded as LTIM) no longer stops with a 404 at the split check. If Yahoo has no price history for a symbol, the check is skipped for it, a note says so, and the rest of the file is imported ([#316](https://github.com/iamsaquib8/stockwise/issues/316)).
- `portfolio xray`: Indian stocks are now called large cap from ₹1,00,000 crore and mid cap from ₹33,000 crore (AMFI's list of December 2024), not ₹20,000 and ₹5,000 crore, so a ₹25,000 crore company shows as small cap and the market-cap split is no longer tilted to large caps ([#100](https://github.com/iamsaquib8/stockwise/issues/100)).
- `import groww` and `import upstox`: a holdings file whose ISIN column holds text such as `₹12,34,567` no longer crashes the import; the cell is treated as not an ISIN ([#102](https://github.com/iamsaquib8/stockwise/issues/102)).
- `import cas`: schemes you have fully redeemed are no longer skipped. When the statement covers every purchase, their redemptions are recorded as realized sales for `tax in`, `tax export in` and `tax advance`, and no holding is left. A redeemed scheme with units bought before the statement is listed as skipped, with the reason ([#90](https://github.com/iamsaquib8/stockwise/issues/90)).
- `import`: a second tradebook for the same broker is merged with the first instead of replacing its lots and sales, so yearly Zerodha Console files can be imported in any order and re-importing a file changes nothing ([#83](https://github.com/iamsaquib8/stockwise/issues/83)).
- `import` and `trade sync`: a buy and sell of the same share on one day are netted as an intraday sale before any older lot is touched, and `tax in`, `tax export in` and `tax advance` report intraday results separately instead of as 111A/112A gains from the oldest lot ([#84](https://github.com/iamsaquib8/stockwise/issues/84)).
- `import` and `trade holdings`: a holdings file from a second broker adds to the shares of the first instead of overwriting them. Lots and sales now carry the demat account (`zerodha`, `groww`, ...), FIFO, imports and syncs work per account, `portfolio lots` shows it, and old portfolio files load unchanged ([#85](https://github.com/iamsaquib8/stockwise/issues/85)).
- `import` and `portfolio add`: dated HDFC and Mindtree trades from before their mergers are now recorded as HDFCBANK (42 for 25) and LTIM (73 for 100) shares at the same total cost and date, instead of the old quantity at the old price ([#86](https://github.com/iamsaquib8/stockwise/issues/86)).
- `tax in`, `tax export in`, `tax advance` and `harvest`: gold, silver, liquid, debt and international ETFs (such as `GOLDBEES`, `LIQUIDBEES`, `MON100`) are no longer taxed as listed shares under Sec. 111A/112A; `tax in` lists them under "taxed under other rules" and `harvest` no longer offers them for the ₹1.25 lakh exemption ([#87](https://github.com/iamsaquib8/stockwise/issues/87)).
- `portfolio`, `import`, `trade sync`, `trade holdings` and `wealth`: commands that change your portfolio or wealth history at the same time no longer overwrite each other's changes. Each takes a lock, reloads the file and applies only its own change; a second command waits up to 30 seconds, then says so ([#93](https://github.com/iamsaquib8/stockwise/issues/93)).
- `deep`: no longer panics (`index out of bounds`) on NSE stocks when the latest bar has a missing close ([#306](https://github.com/iamsaquib8/stockwise/issues/306)).
- Yahoo fetch errors no longer include the request URL, so the session crumb cannot appear in error text.
- `portfolio xray` treats Indian ETFs (such as `NIFTYBEES`, `GOLDBEES`, `LIQUIDBEES`) as funds, also when Yahoo returns no holdings for them, instead of flagging them as one stock with too much weight. Gold, silver and liquid ETFs count as commodity and cash ([#220](https://github.com/iamsaquib8/stockwise/issues/220)).
- `longterm`: the SIP calculator now grows each monthly instalment only for the time it stays invested (₹15,000 a month at 12% gives ₹12.37 L after 5 years, not ₹15.86 L), the "SIP for" target reads `$1M` for US stocks, and the "Suggested SIP Allocation" block is gone ([#67](https://github.com/iamsaquib8/stockwise/issues/67), [#189](https://github.com/iamsaquib8/stockwise/issues/189)).
- Yahoo Finance: commands no longer open the Yahoo session at start-up, so `history`, `backtest` and a provider-backed `quote` still work when Yahoo refuses the session token, and commands that never touch Yahoo send no request at all ([#154](https://github.com/iamsaquib8/stockwise/issues/154)).
- `quote`, `verdict`, `deep`, `score` and `insights`: a symbol Yahoo does not know, such as the BSE code `500325`, now gives a "not found" error with exit code 1 instead of an empty card or a verdict on a company with no price ([#206](https://github.com/iamsaquib8/stockwise/issues/206)).
- `score`, `analyze`, `verdict` and `deep`: companies that file statements in US dollars but trade in rupees, such as Infosys, now have their statement amounts converted at the live USD/INR rate before ratios and revenue are shown, so INFY's Altman Z drops from 426 to about 8 and its revenue reads ₹47.33K Cr, not ₹504 Cr ([#207](https://github.com/iamsaquib8/stockwise/issues/207)).
- `quote`, `compare`, `deep`, `dividends` and `divcal`: dividend yield no longer reads 0 for stocks such as RELIANCE, or 0.05% for INFY, when Yahoo's trailing figure is missing or computed from a dollar dividend; `dividends INFY` now shows 4.4% ([#208](https://github.com/iamsaquib8/stockwise/issues/208)).
- `peers`, `compare`, `deep` and `analyze`: sector, profit margin, debt/equity, revenue growth, beta and analyst ratings are now read from Yahoo's company summary, so `peers TCS` lists IT peers and `compare` no longer shows rows of N/A. Rows and sections with no data at all are left out ([#209](https://github.com/iamsaquib8/stockwise/issues/209)).
- `deep` and `analyze`: banks and lenders no longer show an Altman Z "Distress Zone" next to a strong score; they show "n/a for banks and lenders", and `score` says the same. The doubled brackets in the zone label are gone ([#212](https://github.com/iamsaquib8/stockwise/issues/212)).
- `forecast`: the default history is now 6 months instead of one, so `forecast TCS` prints a projection instead of "Not enough price data". The error now says how many days it found, and the `SYMBOL` argument has a help line ([#215](https://github.com/iamsaquib8/stockwise/issues/215)).
- `watch add`, `portfolio add` and `alert add` now refuse a ticker Yahoo does not know (for example `RELIANC`) and name the likely one, instead of saving it silently; `--force` saves it anyway, and funds, SGBs and offline use are not blocked. `technical`, `history`, `risk` and the other chart commands say "No price history for ZZQQXX.NS" instead of "HTTP 404" ([#216](https://github.com/iamsaquib8/stockwise/issues/216)).
- `portfolio` and `portfolio --json` now work offline: they list every holding with shares, average cost and cost basis, show prices as N/A (`price: null` and `unpriced` in JSON) and say "Offline: prices unavailable" instead of failing. `tax`, `harvest`, `rebalance` and `xray` still need a connection ([#221](https://github.com/iamsaquib8/stockwise/issues/221)).
- `alert watch` and `alert check`: `change-above`, `change-below` and `volume-spike` alerts no longer
  repeat the last session's move on weekends, holidays and before the open (for example Friday's fall
  announced again on Saturday, Sunday and Monday morning by `alert watch --once` in cron). They give
  no reading until today's session has traded and re-arm on the quote's own session date, so a crypto
  alert does not fire twice across midnight in your market's time zone
  ([#103](https://github.com/iamsaquib8/stockwise/issues/103)).
- `sim replay end` now scores and clears a replay whose price file (`sim_replay_tape.json`) was
  deleted, instead of asking you to run `sim replay end` and leaving the replay stuck
  ([#106](https://github.com/iamsaquib8/stockwise/issues/106)).
- `sim history` and `sim status` no longer mix markets: history totals, best and worst day, win rate,
  streaks and the chart cover only the `-m` market (rupees and dollars were added together), and `sim
  status` shows each session in its own currency. `sim start` is no longer blocked by an open session
  of another market the same day ([#107](https://github.com/iamsaquib8/stockwise/issues/107)).
- `sim` practice score: the drawdown part is now judged against what the benchmark would have lost at
  your average exposure, so holding mostly cash through a crash no longer earns drawdown points that
  the same picks fully invested would not. Scores of runs with under full exposure can drop by up to
  about 14 points ([#119](https://github.com/iamsaquib8/stockwise/issues/119)).
- `trade buy`, `trade sell`, `trade limit` and `trade gtt` with `--yes` are now refused, before any
  broker login, unless `max_order_value` is set in the broker's config file, since `--yes` removes the
  only other guard against a wrong quantity. Scripts that use `--yes` need `"max_order_value": 50000`
  (or your own cap) added; `--dry-run` and typing `yes` at the prompt work as before. The preview now
  notes when no cap is set ([#125](https://github.com/iamsaquib8/stockwise/issues/125)).
- MCP `order_preview`: the `stockwise trade` command it returns now quotes symbols with `&`, such as
  `'M&M'`, so pasting it into bash, zsh or PowerShell no longer splits at the `&`
  ([#126](https://github.com/iamsaquib8/stockwise/issues/126)).
- `ask`: a failing tool no longer sends the full error to the model. Local file paths (a corrupt
  `portfolio.json` includes your home directory) and the Yahoo session `crumb` are removed, as
  `stockwise mcp` already did; `--verbose` still prints the full error on your terminal. The notice
  before the first cloud request now says that your portfolio may be sent if a tool reads it, not only
  market data ([#127](https://github.com/iamsaquib8/stockwise/issues/127)).
- `correlate` and `pcorr` pair returns by date instead of by position, so symbols with different holiday calendars (NSE and NYSE) or a seven-day crypto week no longer give a correlation near zero. Two symbols with fewer than 20 shared returns show `N/A`, and `pcorr` no longer prints `0.000` for them ([#72](https://github.com/iamsaquib8/stockwise/issues/72)).
- `risk` and the volatility in `timeframes` annualize with the right number of bars for the asset: 365
  a year for crypto, 260 for FX, 252 for stocks, and 288 five-minute bars a day for crypto. `returns`
  annualizes by calendar time and shows `—` (null in JSON) for spans under a year, so a +100% year
  reads +100% for crypto and for stocks. Calmar annualizes over the number of returns
  ([#73](https://github.com/iamsaquib8/stockwise/issues/73)).
- Pivot points in `support`, `intraday`/`bot` and the stock report come from the last completed daily session, not today's live bar, a weekly bar or the last five minutes, so the levels no longer move with every tick or change with `-p`. `support` names the session they come from ([#74](https://github.com/iamsaquib8/stockwise/issues/74)).
- `options oi` reads put build-up for the underlying: put buying (premium and open interest up) is now bearish and put writing is bullish, and the per-strike tags are coloured that way. `--json` gains `calls_sentiment` and `puts_sentiment` ([#75](https://github.com/iamsaquib8/stockwise/issues/75)).
- `longterm`, `verdict` and `insights` compare market cap, cash and insider trades with cut-offs
  written in US dollars after converting from the quote currency (at rough fixed rates), so the same
  size scores the same for a rupee and a dollar stock: a ₹1.5 lakh insider buy no longer earns the
  insider-buying bonus, and ₹5,000 crore is a small cap
  ([#76](https://github.com/iamsaquib8/stockwise/issues/76)).
- `tax in`, `tax export in`, `tax advance` and `harvest` now decide in one place which sales count as
  listed shares or equity funds (Sec. 111A/112A). Debt funds, SGBs, crypto and gold, silver, liquid
  and international ETFs are left out of all four, so a tax-free SGB redemption no longer raises
  advance tax and a crypto loss no longer reduces STCG tax
  ([#88](https://github.com/iamsaquib8/stockwise/issues/88)).
- `tax advance` no longer shows past instalments as `Paid` when no payment is recorded: they are `Overdue`. `--paid` takes dated payments (`--paid 2025-06-14:3000`, repeated) so Sec. 234C interest follows what was paid by each due date, on shortfalls rounded down to ₹100 ([#89](https://github.com/iamsaquib8/stockwise/issues/89)).
- `harvest` offers losses as the FIFO sale you would actually make: a losing lot behind older lots is
  sold with them, and a holding whose sale books a net gain is listed as FIFO-blocked instead of a
  loss. Tax saved comes from the tax estimate with this year's gains, the shared ₹1.25 lakh exemption
  is split between candidates, and `Total P&L` shows its real sign
  ([#91](https://github.com/iamsaquib8/stockwise/issues/91)).
- `harvest` no longer counts US holdings as rupees in the Indian harvest: they are listed under `other_currency`, as `tax in` does ([#92](https://github.com/iamsaquib8/stockwise/issues/92)).
- `tax in`, `tax export in`, `harvest` and `tax advance` now include the 4% health and education cess in India estimates, so totals and advance-tax instalments are no longer about 4% too low. Surcharge is still left out ([#98](https://github.com/iamsaquib8/stockwise/issues/98)).
- `tax export in` applies the rates and exemption that were in force on each sale date: sales before 23 Jul 2024 are taxed at 15% (STCG) and 10% (LTCG), and the LTCG exemption is ₹1 lakh up to FY 2023-24. A mixed FY 2024-25 shows how much of the taxable gain came from before 23 Jul ([#99](https://github.com/iamsaquib8/stockwise/issues/99)).
- `daemon intraday` keeps running after it settles a session and waits for the next trading day (weekends and exchange holidays skipped), so one process covers every day and `stockwise_daemon_up` stays 1. Use the new `--once` flag for one session and exit ([#117](https://github.com/iamsaquib8/stockwise/issues/117)).
- `/metrics` no longer reports a finished day's daemon positions as open: a settled state shows `stockwise_daemon_open_positions 0`. The money gauges carry a `currency` label (`INR`, `USD`) and `stockwise_daemon_last_update_timestamp_seconds` is new ([#120](https://github.com/iamsaquib8/stockwise/issues/120)).
- `fibs` always reads a year of daily bars and reports the last 21, 63 and 126 trading days, so its 1, 3 and 6 month sections no longer show the same levels; `-p` is now ignored ([#137](https://github.com/iamsaquib8/stockwise/issues/137)).
- `stockwise C -m us` and the other single-capital-letter US tickers (`T`, `D`, `A`, `W`) are read as symbols instead of failing with "unrecognized subcommand"; the lowercase aliases such as `c` for `compare` still work, and `stockwise quote C` is the explicit form ([#147](https://github.com/iamsaquib8/stockwise/issues/147)).
- `serve`, `tui` and `mcp` get a new Yahoo session when Yahoo answers HTTP 401 to a quote request, instead of failing until restarted: one fresh handshake (at most once a minute, shared by concurrent requests), then the request is retried once. Yahoo's cookies are now kept and refreshed by the HTTP client rather than copied by hand ([#155](https://github.com/iamsaquib8/stockwise/issues/155)).
- `serve`, `tui`, `mcp` and the daemons keep using Alpha Vantage, Massive or Twelve Data after their free-tier request allowance runs out or a request fails: the allowance refills each window (a minute, or 4 h 48 min for Alpha Vantage) and a pause after an error ends after a few minutes, with one notice per pause ([#157](https://github.com/iamsaquib8/stockwise/issues/157)).
- With Alpha Vantage, Twelve Data or Massive, a quote keeps the provider's trade date instead of Yahoo's time of today, so alert checks no longer treat yesterday's close as a bar for today ([#158](https://github.com/iamsaquib8/stockwise/issues/158)).
- `-m ca` closes the market on New Year's Day, Good Friday and the other TSX holidays in 2027 (provisional until TMX publishes its list), and the NSE 2027 list includes Good Friday, 26 March ([#164](https://github.com/iamsaquib8/stockwise/issues/164)).
- `mf` commands and `MF:` holdings use the saved AMFI file, and say which day it was saved, when AMFI answers with a maintenance page or an empty body instead of the NAV data ([#166](https://github.com/iamsaquib8/stockwise/issues/166)).
- `risk`, `backtest` and `practice` compute the Sortino ratio with downside deviation taken over all periods instead of only the losing ones, and measure shortfall against the risk-free rate, so strategies with few down days are no longer scored too low ([#28](https://github.com/iamsaquib8/stockwise/issues/28)).
- Terminal charts print "Chart dimensions too small" instead of panicking when the terminal width or height is zero or too small to draw them ([#30](https://github.com/iamsaquib8/stockwise/issues/30)).
- Importing a spreadsheet with a corrupt cell reference (a column past `XFD`) skips that cell instead of allocating gigabytes or crashing ([#33](https://github.com/iamsaquib8/stockwise/issues/33)).
- `import` of a demat CAS puts BSE scrip codes such as `500325` on `.BO` instead of `.NS`, and no longer doubles a `.NS` or `.BO` suffix that casparser already wrote ([#34](https://github.com/iamsaquib8/stockwise/issues/34)).
- Cached pages and reference files are written through a temp file with its own name per write, so two requests saving the same page at once can no longer corrupt it or fail the rename; a failed write leaves no temp file behind ([#36](https://github.com/iamsaquib8/stockwise/issues/36)).
- Several NSE or BSE requests that meet an expired session at once now share one session refresh, so the exchange is no longer treated as having refused us and skipped for 15 minutes ([#40](https://github.com/iamsaquib8/stockwise/issues/40)).
- `risk` shows no Calmar ratio, rather than `NaN` or `inf`, when a price series starts at zero or ends at or below zero, and the maximum drawdown ignores prices until the series first turns positive ([#43](https://github.com/iamsaquib8/stockwise/issues/43)).
- `sim history` draws a one-cell bar for a breakeven day instead of a full-width bar when the best and worst day are both zero ([#44](https://github.com/iamsaquib8/stockwise/issues/44)).
- `daemon intraday` reports a position's return from its total profit, booked half included, over what it cost, so a position that booked Target 1 no longer shows a wrong or negative percentage and `sim history` stores the right figure ([#56](https://github.com/iamsaquib8/stockwise/issues/56)).
- `score`: when a company reports no net income, the Piotroski net income is estimated from trailing EPS
  times shares, or from profit margin times revenue, and marked "(est.)". It was profit margin times market
  cap, which overstated it by the price-to-sales ratio and could cost the cash-flow-above-earnings point
  ([#68](https://github.com/iamsaquib8/stockwise/issues/68)).
- `score`: the Altman Z-score needs all five ratios. With any missing, `score` shows "n/a", names the
  missing factors and gives no zone, instead of summing the ratios it had. Guessed inputs (a fixed share
  of assets as current assets, 70% of equity as retained earnings, ROA in place of EBIT) are removed, and
  Z'' is still given when only the sales ratio is missing
  ([#69](https://github.com/iamsaquib8/stockwise/issues/69)).
- `score`: the non-manufacturing Altman Z'' now uses book equity over total liabilities for its fourth
  ratio, as the model defines it, and shows its own factor table. It used market cap, which put high
  price-to-book stocks in the Safe zone whatever their other ratios said
  ([#70](https://github.com/iamsaquib8/stockwise/issues/70)).
- `score`: the Piotroski F-Score counts only the signals that have data. A missing share count, prior year
  or margin shows "n/a", earns no point and is left out of the count (for example 5/7 evaluated), and a
  rating is given only when all nine signals ran. Gross margin is compared with gross margin, or operating
  margin with operating margin, for both years. `--json` adds `evaluated`; `rating` is null when not rated
  ([#71](https://github.com/iamsaquib8/stockwise/issues/71)).
- `tax advance --paid` ignores a payment dated before the financial year began, since it was made for an earlier year, and says so in the output.
- `tax advance --fy` for a year before 2024-25 uses the rates and exemption that applied to each sale date, as `tax export in` does: STCG of Rs 1,00,000 in FY 2023-24 is now Rs 15,600 with cess, not Rs 20,800.
- `tax advance` no longer charges Sec. 234C interest on the last instalment for gains made after 15 March when their tax is paid by 31 March; the 15 March requirement now covers only gains up to that day.
- Alpha Vantage: StockWise now sends at most 4 requests per 4 h 48 min window instead of 5, because
  two windows can sit back to back and 5 could put 29 requests into one day against the free plan's
  25.
- `levels` and `alert`: a crypto bar stamped after 21:00 UTC is no longer dated the next day, so the
  daily bar and the prior-session pivots for crypto no longer shift by a day in the evening.
- `daemon intraday`: Ctrl+C or SIGTERM on a day that is already settled no longer closes its leftover open rows at stale prices or adds made-up P&L; `--once` on a settled day exits at once instead of waiting for the next session; and a saved state with open positions but no entry flag no longer loops printing "Session complete".
- `tax in` no longer treats a sale as intraday just because a lot with an estimated purchase date (from a holdings snapshot or sync) is dated the same day; those sales stay in the 111A/112A figures.
- `harvest` values funds you sold in full like `tax in` does, so their gains use up the Rs 1.25 lakh exemption it reports instead of being dropped.
- `import` of a file with dated buys and no sells no longer silently deletes the sales and kept trades an earlier tradebook of the same broker recorded for that symbol; it skips the symbol and asks for `--replace`.
- `trade`: a quote of 0 (a halted stock) or an invalid price no longer counts as a price, so a market order cannot pass `max_order_value` as worth nothing. With a cap set and no usable price the order is refused with "Cannot estimate the order value".
- MCP `order_preview`: a symbol that StockWise's quote lookup rewrites (such as `HDFC` or `L&T`) is refused with the NSE ticker to use (`HDFCBANK`, `LT`), so the preview always prices the instrument the `trade` command will send. For quoted symbols it also tells Windows `cmd.exe` users to use double quotes.
- `import` and `portfolio add` no longer apply HDFC Bank's 2019 split a second time to HDFC (or Mindtree) lots restated after the merger: 25 HDFC bought in 2018 stay 42 HDFCBANK shares at the same total cost, and only splits and bonuses after the merger date are applied.
- `mf nav` and `mf history`: when a download fails, a saved copy is used only if it is at most 7 days
  old (older is an error that says so), and the output says it is a saved copy and from which day,
  with a `stale` entry in `--json`. Before, any age was accepted and only a line on stderr said so.
- `research` and `verdict`: the NSE shareholding read asks for at most 8 filings, even when many of the
  listed ones do not load, instead of working through the whole list of dozens.
- `research` and `verdict`: the NSE shareholding read now times out per request (3 seconds each, 8
  seconds in all) and keeps the quarters it has read, saying how many in a warning, instead of dropping
  all of them when a cold start is slow. The verdict no longer loses its shareholding pillar for that.
- `levels`, `deep`, `technical` and `intraday`: a symbol's own market now decides whether today's
  session is still running and how many bars make a year, instead of the market you ran the command
  with (India by default). `levels BTC-USD` or `levels ^GSPC` on a weekday evening no longer builds
  pivots from today's half-finished bar and calls them the prior session.
- `research`, `analyze` and `score`: the newest quarter and the balance sheet that come from Yahoo's
  fundamentals summary are now converted to the share's currency like the other statements, so Infosys
  (reported in US dollars, quoted in rupees) no longer shows about -99% quarterly revenue growth or
  wrong margins. With no exchange rate, those amounts are left out.
- `research`, `score` and `verdict`: total debt from the balance sheet now adds the current portion to
  long-term debt, so a company with nothing due this year but long-term loans no longer looks debt-free.
  Zero counts as real only when both are reported as zero.
- Reading an expense-ratio spreadsheet no longer reserves memory from the file's own size claim, and refuses a sheet that inflates past 64 MiB, so a crafted `.xlsx` cannot crash the program.
- Yahoo Finance: when the cookie and crumb handshake fails (for example while Yahoo is rate limiting you),
  StockWise now remembers the failure for 45 seconds instead of repeating up to six requests and their
  waits for every later call, so a command that makes many fundamentals calls fails fast.
- Prices in Swiss francs, South Korean won, New Taiwan dollars, Chinese yuan and
  Brazilian reais are shown with `CHF `, `₩`, `NT$`, `CN¥` and `R$` instead of
  `$` ([#17](https://github.com/iamsaquib8/stockwise/issues/17), thanks
  @O1sumitkumar).
- `movers` lists only rising stocks under gainers and only falling stocks under
  losers, and says so when a side is empty
  ([#210](https://github.com/iamsaquib8/stockwise/issues/210), thanks @ShahadatTest).

### Performance
- `ask`: when the model asks for several tools in one turn, up to 4 of them now run at the same time
  instead of one after another, and the results still go back in the model's order. The per-turn tool-call
  limit counts each call as before ([#113](https://github.com/iamsaquib8/stockwise/issues/113)).
- Release builds strip symbols and use one codegen unit, so the downloaded binary is about 28% smaller
  (20.5 MB to 14.7 MB on macOS arm64) ([#177](https://github.com/iamsaquib8/stockwise/issues/177)).

### Security
- `ask`, `report` and `--ai`: plain `http://` to an Ollama server on another machine is refused unless `STOCKWISE_ALLOW_REMOTE_OLLAMA=1` is set, because the prompts carry holdings and tax figures. `ask` also prints a notice whenever the Ollama server is not on this computer, as it already did for cloud providers.

## [0.8.0] - 2026-11-03

### Added
- `portfolio xray` shows your real exposure to each company across direct holdings, mutual funds
  and ETFs, flags any company above 10% of the portfolio, and splits the portfolio by asset class,
  market cap and sector ([#51](https://github.com/iamsaquib8/stockwise/issues/51)).
- `score` shows the Piotroski F-score and the Altman Z-score with every input and formula, and
  notes when a model does not suit a bank ([#50](https://github.com/iamsaquib8/stockwise/issues/50)).
- `options oi` classifies open-interest changes as long build-up, short covering, short build-up
  or long unwinding, with PCR and max pain; `options chain` shows the same tag per strike ([#49](https://github.com/iamsaquib8/stockwise/issues/49)).
- `tax advance` lists the four advance-tax instalments with what is due, and estimates Sec. 234C
  interest on shortfalls ([#46](https://github.com/iamsaquib8/stockwise/issues/46)).
- `portfolio target` sets target weights by stock or sector. `rebalance --inflow` puts new money
  only into underweight holdings, and `--drift` ignores small deviations ([#47](https://github.com/iamsaquib8/stockwise/issues/47)).
- `ipo` shows open, upcoming and recently listed IPOs from NSE, and for one issue: subscription by
  category, allotment odds, anchor and lead managers, and a scored summary. Grey-market premiums
  are shown only when news headlines report them, labelled unofficial.

- Faster everyday use ([#54](https://github.com/iamsaquib8/stockwise/issues/54), [#55](https://github.com/iamsaquib8/stockwise/issues/55)):
  - `stockwise SYMBOL` with no command prints a one-screen overview, and bare `stockwise` prints a quick start.
  - Short names: `q` (quote), `d` (dashboard), `a` (analyze), `t` (technical), `c` (compare), `p` (portfolio), `w` (watch).
  - Common names resolve to NSE symbols (`SBI` → `SBIN`, `L&T` → `LT`, and index names), and `quote` suggests close matches for a mistyped symbol.
- `--explain` on `verdict`, `technical`, `score` and `options oi` adds plain-English notes for beginners ([#52](https://github.com/iamsaquib8/stockwise/issues/52)).
- `harvest --json` / `--csv` and lot-level loss and gain candidates ([#21](https://github.com/iamsaquib8/stockwise/issues/21)).
- Broker `.xlsx` exports import directly, reading every sheet and Excel date cells ([#45](https://github.com/iamsaquib8/stockwise/issues/45)).
- Eight more read-only MCP and `ask` tools: `verdict`, `scorecard`, `insights`, `events`, `options_oi`, `tax_advance`, `harvest` and `rebalance`.
- Web dashboard: a research workbench, a portfolio switcher and keyboard shortcuts. TUI: a stock inspector and a sort key.
- `--base` accepts any three-letter currency code.

### Changed
- MACD is computed in one pass instead of recomputing the EMAs for every bar, so long histories no longer slow it down ([#29](https://github.com/iamsaquib8/stockwise/issues/29)). Thanks [@immconcon](https://github.com/immconcon).

### Fixed
- Guard against zero or unavailable price in `verdict` upside percentage calculation, preventing `+inf%` output and invalid JSON serialization on unpriced stocks ([#35](https://github.com/iamsaquib8/stockwise/issues/35)).
- Portfolio totals in a base currency other than USD or INR used the wrong side of the FX pair. Conversions now follow Yahoo's quotation direction ([#32](https://github.com/iamsaquib8/stockwise/issues/32)). Thanks [@AndreIsraelAO](https://github.com/AndreIsraelAO).
- The EMA no longer divides by zero when asked for a period of 0.
- Percent returns in `markets`, technicals and risk metrics no longer show `+inf%` when the first price is zero or negative ([#59](https://github.com/iamsaquib8/stockwise/issues/59)). Thanks [@ShahadatTest](https://github.com/ShahadatTest).

### Known issues
Fixed in 0.8.1. Check the linked issues before relying on the numbers involved.
- Importing a second tradebook or holdings file from a broker can replace earlier lots ([#83](https://github.com/iamsaquib8/stockwise/issues/83), [#85](https://github.com/iamsaquib8/stockwise/issues/85)), and same-day trades are booked as capital gains ([#84](https://github.com/iamsaquib8/stockwise/issues/84)). Back up `portfolio.json` before importing.
- `tax` treats gold, silver, liquid and international ETFs as listed shares ([#87](https://github.com/iamsaquib8/stockwise/issues/87)).
- The SIP projection in `longterm` overstates the corpus ([#67](https://github.com/iamsaquib8/stockwise/issues/67)).
- Statements reported in USD (for example INFY) are shown in ₹ in `research` scorecards ([#207](https://github.com/iamsaquib8/stockwise/issues/207)).

## [0.7.0] - 2026-11-01

### Added
- `events <SYMBOL>` (`ev`): a timeline of company announcements from NSE and news feeds, with
  figures pulled from the text, the market's reaction against NIFTY 50 or the S&P 500, and
  upcoming events.
- `insights <SYMBOL>`: positive, negative and neutral findings across nine areas (ownership,
  growth, margins, valuation, balance sheet, analysts, momentum, dividends, news), with NSE
  shareholding data for India and SEC insider and institutional data for the US.
- `verdict <SYMBOL>`: a 0–100 score from six weighted pillars with a BUY-to-SELL label, entry
  zone, stop-loss and target. These labels are being replaced with neutral scores ([#188](https://github.com/iamsaquib8/stockwise/issues/188)).

### Fixed
- Windows main-thread stack overflow (`thread 'main' has overflowed its stack`) by running the CLI entrypoint and Tokio worker threads on an explicit 8 MiB stack and heap-pinning the command dispatch state machine.

## [0.6.0] - 2026-10-26

Not released separately: these changes first shipped in 0.7.0.

### Added
- A documentation site at <https://iamsaquib8.github.io/stockwise/> with
  search, a one-page [cheatsheet](docs/cheatsheet.md) (a test checks that every
  command in it parses) and an honest [comparison](docs/comparison.md) with
  other tools.
- `stockwise mcp` — a [Model Context Protocol](https://modelcontextprotocol.io)
  server for Claude Desktop, Claude Code, Cursor and other AI assistants, on
  the official Rust SDK (`rmcp`). 24 read-only tools: `quote`, `history`,
  `technical`, `risk`, `search`, `markets`, `mf_search`, `mf_nav`, `mf_sip`,
  `options_chain`, `options_pcr`, `options_maxpain`, `options_payoff`,
  `screen`, `backtest`, `portfolio_summary`, `holdings_lots`, `tax_estimate`,
  `calendar`, `goal_sip`, `goal_retire`, `goal_education`, `alerts_list` and
  `watchlist`, each returning the same data as the command's `--json`, with a
  "not financial advice" note and results kept small. Resources: the
  portfolio, watchlist and alerts. Prompts: `review_portfolio`,
  `tax_harvest_check` and `explain_option_chain`. Stdio by default;
  `--http 127.0.0.1:8788` serves Streamable HTTP at `/mcp` (loopback only,
  or `--allow-remote` with a `STOCKWISE_API_TOKEN` bearer token, like
  `serve`). `--list-tools` prints the tools and their JSON schemas. Behind
  the default-on `mcp` cargo feature.
- No orders from AI assistants: the MCP server never places, changes or
  cancels orders and never reads broker credentials. `--allow-orders` only
  adds `order_preview`, a dry run that checks an order against the last price
  and returns the `stockwise trade` command for you to run (with its own
  preview, value cap and typed `yes`).
- `stockwise ask "question"` — the configured model (local Ollama by default;
  OpenAI, Anthropic or Gemini when `STOCKWISE_AI_PROVIDER` selects one)
  calls the same tools through each provider's tool calling and answers in
  plain English. Model requests, tool calls (12) and tokens are capped
  (`--max-steps`, `--max-tokens`); the last request runs without tools.
  `--verbose` shows each tool call, `--json` prints the transcript. A model
  without tool calling gets an error naming one that has it.
- Findable by AI assistants: the docs site publishes `llms.txt` (the pages
  and the cheatsheet's commands), `llms-full.txt` and a Markdown copy of each
  page; `skills/stockwise/SKILL.md` is an Agent Skill for Claude Code and
  other agents; `server.json` describes the server for the official MCP
  registry (`io.github.iamsaquib8/stockwise`: crates.io, PyPI, GHCR), and
  `packaging/mcpb/` builds a Claude Desktop extension (`.mcpb`).
- **`pipx install stockwise` / `uvx stockwise`** — PyPI wheels that carry the
  prebuilt binary (maturin `bindings = "bin"`, no Python code or
  dependencies) for manylinux and musllinux x86_64/aarch64, macOS x86_64 and
  arm64 and Windows x86_64, plus an sdist that builds from source. Built on
  every tag; uploaded with PyPI trusted publishing once the maintainer
  enables it.
- **`.deb` and `.rpm` release assets** for x86_64 and arm64
  (`stockwise_amd64.deb`, `stockwise-x86_64.rpm`, ...), with man pages, bash/
  zsh/fish completions and docs, and `.sha256` files. Version-less names, so
  `releases/latest/download/...` links stay stable; `sudo dnf install <URL>`
  works directly.
- **Termux on Android**: an `aarch64-linux-android` release archive (NDK, API
  24), picked by `install.sh` on Termux (installs to `$PREFIX/bin`) and by
  `cargo binstall`; and a ready-to-submit termux-packages recipe in
  `packaging/termux/`.
- **nixpkgs**: a ready-to-submit `packaging/nixpkgs/package.nix`
  (`buildRustPackage` with `cargoHash`, man pages, completions, version check,
  update script), built in CI with the flake's nixpkgs.
- **Docker Hub mirror**: release tags copy the multi-arch GHCR image to
  `docker.io/iamsaquib8/stockwise` with its description, when Docker Hub
  credentials are configured. Both registries now carry an OCI description
  on the multi-arch index.
- **Download stats without telemetry**: a nightly workflow archives crates.io,
  GitHub release (per asset), GHCR, Docker Hub, PyPI, Homebrew, AUR and GitHub
  traffic/referrer numbers to the `stats` branch;
  `scripts/download-stats.py summary` prints them. Nothing is collected from
  users.
- An [Install](docs/install.md) page listing every install path, what it
  includes and how it updates.

### Changed
- Mutual funds and Sovereign Gold Bonds are valued in `portfolio` (table,
  `--json`, `--csv`), `dashboard`, `tui`, `serve` / the web dashboard,
  `wealth`, `rebalance`, `harvest`, `tax`, `report portfolio` and the long-term
  daemon. `MF:<AMFI code>` and `MF:<ISIN>` (growth/payout or IDCW-reinvestment
  ISIN) use AMFI's latest end-of-day NAV, shown as "NAV as of &lt;date&gt;"; SGBs (NSE
  symbol or tranche ISIN) use NSE's last price, or one gram of gold at spot
  labelled "estimated" when NSE refuses (the 2.5% interest is left out, and
  said so). An unknown code stays N/A with a note, never 0. `portfolio --json`
  rows gain `name`, `price_source` and `nav_date`, and the report a `notes`
  list. `watch` and `alert` see fund NAVs too, `portfolio sell` defaults to
  the NAV, and `MF:` symbols are no longer sent to Yahoo.
- `tax in` counts equity-oriented mutual funds (by AMFI category) at the
  listed-share rates and names them; debt and other funds and SGBs are left
  out of the estimate with the rules that apply to them (`equity_funds` and
  `other_rules` in `--json`), instead of silently getting equity rates.
- AI sections (`report`, `--ai`) give neutral, educational analysis — signals,
  risks, what to watch — instead of BUY/HOLD/SELL verdicts, picks or
  allocation advice, in line with SEBI's adviser, research-analyst and
  finfluencer rules. Section titles say "notes" and "comparison" rather than
  "verdict", "memo" or "brief", and every AI section ends with a one-line
  disclaimer. Cache keys include a prompt version, so answers cached under the
  old wording are not shown again.

### Fixed
- `portfolio add MF:120503`, `watch add` and `alert add` stored `MF:120503.NS`
  (`-USD`, `.L`, ... in other markets), which nothing could price. `PREFIX:ID`
  symbols are now kept as typed in every market, and symbols stored that way
  by older versions are repaired when the portfolio, watchlist and alerts load
  (a holding that then appears twice is merged).
- `MF:` alerts follow Indian market hours instead of US ones.
- `portfolio actions` skips funds and SGBs instead of asking Yahoo for them;
  `portfolio benchmark` and `tax export` handle funds stored as `MF:<ISIN>`;
  `portfolio sell` without a price refuses an SGB valued only by its gold
  estimate.
- `report portfolio` no longer gives the AI a price of 0 for holdings without
  a live price, and `export portfolio` / `export watchlist` leave a missing
  price empty instead of writing 0.
- The rule-based insights in `analyze` were titled "AI Insights"; they are not
  AI and are now titled "Insights".
- The Docker image carries the `io.modelcontextprotocol.server.name` label the
  MCP registry checks.
- Prebuilt Linux binaries (archives, `install.sh`, `cargo binstall`, Homebrew
  on Linux, AUR) required glibc 2.39 and failed on Ubuntu 22.04, Debian 12 and
  RHEL 9 with "version `GLIBC_2.39' not found". They are now built on Ubuntu
  22.04 and need glibc 2.34 or newer.

## [0.5.0] - 2026-10-25

Planning, reporting and more markets: tax exports, benchmarks and goal
planning, technical alerts and a results calendar, NSE/BSE data, more
exchanges and commodities, a web dashboard, a simulator you can play,
broker sync with a read-only self-test, and WASM strategy plugins. The
minimum Rust version is now 1.97. Command-line forms from 0.4 keep working.

### Added
- `stockwise tax export [in|us] [--fy 2025-26] [--out FILE] [--json]` — the
  capital-gains schedule of a tax year's realized sales. India: one row per
  lot sold, laid out for ITR Schedule CG / Schedule 112A (section 111A/112A,
  ISIN from NSE's equity list or AMFI, scrip, quantity, sale value, cost, days
  held, gain), then the summary — STCG, LTCG, set-off, the ₹1.25 lakh
  exemption, estimated tax, carry-forward. US: a Form 8949 worksheet (Part I /
  Part II) with Schedule D totals and the $3,000 loss deduction. Labelled an
  estimate to verify against your broker's tax P&L; flags grandfathered
  (pre-Feb-2018) shares, pre-23-Jul-2024 sales and estimated purchase dates.
- `stockwise portfolio benchmark [NIFTY|SENSEX|SP500|symbol] [-p 1y]` — the
  portfolio's time-weighted return against an index, rebuilt from the lots held
  at each close; alpha, beta, tracking error, information ratio and correlation
  from daily returns; attribution by holding and by sector (Carino-linked, so
  contributions add up to the totals); a growth-of-100 chart; other currencies
  converted at daily exchange rates; holdings with partial history flagged.
  `--json` / `--csv`.
- `stockwise goal sip|retire|education` — goal planner: the monthly SIP for a
  target corpus (with a yearly step-up and existing savings), the retirement
  corpus for today's monthly expenses and the SIP that builds it, and an
  education cost grown by inflation. Amounts in Indian units (`50k`, `25L`,
  `1cr`), results in lakhs and crores; `--json`. New **Planning** help group.
- `stockwise trade selftest [--broker NAME] [--redact-amounts] [--json]` — a
  strictly read-only check of a broker integration against your own account:
  login / session, profile, holdings, positions, order book, trade book, GTT
  list, exact instrument lookup for RELIANCE, TCS and NIFTYBEES, a 1-share
  LIMIT buy run through every order check without being sent, and
  response-shape checks (missing and new field names, never values). Prints a
  redacted Markdown report for a GitHub issue: no credentials or tokens, the
  account id cut to its last 2 characters, e-mails and long numbers masked in
  error messages, amounts hidden with `--redact-amounts`. The session is
  wrapped so that every order, GTT or cancel call fails before reaching the
  broker. See [Help verify a broker](docs/brokers.md#help-verify-a-broker).
- Community testing kit: a "Broker test report" issue template, a
  verification-status table per broker in docs/brokers.md, and the criteria for
  dropping the experimental label (3 independent self-test reports plus one
  small live order a tester chose to place themselves).
- `stockwise trade sync [--since DATE] [--apply] [--notify] [--json]` — pulls
  the broker's executed trades into the lot-based portfolio: delivery buys
  become dated lots (fill date, average fill price, broker order id), sells
  come out FIFO as realized sales. Previews by default; `--apply` writes.
  Idempotent: applied fills are remembered by broker, order and trade id, so re-running,
  or syncing the rest of a partly filled order, never double-counts. Intraday,
  margin and F&O fills are skipped; sells wait while a split or bonus is not
  yet applied to the lots; open and partly filled orders are listed;
  quantities are compared with the broker's holdings. Dhan reads older fills
  with `--since`; the other brokers return today's trades only.
- `trade gtt list` shows what changed since the last look (triggered,
  cancelled, expired, no longer listed), with `--notify` to send the changes
  to your notification channels and `--json`. `trade sync --apply` tracks the
  same changes.
- `Broker` trait: `profile`, `trades` (trade book), raw read-only `rows` and
  per-endpoint response shapes for all five brokers; order-book rows now carry
  filled quantity, average price and product. Experimental on Zerodha, Upstox,
  Dhan and Fyers like the rest of those integrations — the new endpoints were
  built from the brokers' public docs and tested against fixtures only.
- Portfolio lots and realized sales record the broker order id they came
  from (`order_id`); the portfolio file keeps the list of synced fills
  (`broker_fills`). Older files load unchanged.
- Practice mode for the paper-trading simulator ([docs/simulator.md](docs/simulator.md)):
  - `sim replay`: trade a past market day by day with the future hidden. Six
    built-in scenarios (`covid-crash-2020`, `gfc-2008`, `adani-2023`,
    `dotcom-2000`, `bull-run-2021`, `rate-hikes-2022`), your own window
    (`--from --to --symbols`) or a scenario file. Daily history is fetched once
    and every view, `--json` included, stops at the replay date. Orders fill at
    the next open with the backtest cost models; stops and targets are checked
    on each day's range.
  - `sim challenge`: goal-based challenges with hard and soft rules — beat the
    index, keep drawdown under 10%, no position above 20%, always set a
    stop-loss, SIP discipline vs timing, survive the crash — over a replay or
    over live paper sessions. Rules are data (TOML).
  - A 0–100 score and grade from risk-adjusted return against the benchmark
    (Sharpe, Sortino), drawdown control, discipline and diversification —
    never raw return alone — plus badges and streaks (`sim profile`).
  - `sim card [--svg FILE]`: a shareable text or SVG result card. Paper
    money only; every scorecard and card carries the disclaimer.
- Web dashboard at `/` of `stockwise serve`: indices, watchlist, portfolio
  (P&L, XIRR, allocation, INR/USD totals, sortable columns), alerts, symbol
  search and a price chart with period buttons and a keyboard-readable
  crosshair. Refreshes every 30 seconds while visible; works at phone width;
  follows the system light/dark theme; amounts in lakhs and crores for INR.
  Plain HTML/CSS/JavaScript compiled into the binary — no build step or CDN.
  Read-only. See [docs/server.md](docs/server.md#web-dashboard).
- `stockwise serve --open` opens the dashboard in your browser; the startup
  log prints its address.
- Expense ratios for Indian mutual funds from AMFI's monthly TER disclosure:
  `mf nav` and `mf history` show the plan's total expense ratio (and the
  other plan's), `mf screen` adds a TER column, `--sort ter` (lowest first)
  and `--max-ter PERCENT`. The disclosure is downloaded once a day (~3 MB
  spreadsheet export) and cached in `cache/amfi_ter.json`; `--json` adds a
  `ter` object with the April 2026 breakdown (base expense ratio, brokerage,
  transaction costs, statutory levies). When AMFI can't be reached, NAVs and
  returns still print with a note.
- BSE options: `options chain|pcr|maxpain SENSEX` (also `BANKEX`, `SENSEX50`,
  `^BSESN`) and BSE stock options as `SYMBOL.BO`, from BSE's public
  derivatives API with one session, spaced calls, a one-minute cache and a
  clear error when BSE refuses scripted requests. NSE indices and stocks stay
  on NSE, `-m us` on Yahoo.
- Lot sizes in option chains: inferred from NSE/BSE order-book quantities
  (always whole lots), 100 for US contracts; `options chain` shows them and
  adds `lot_size` to `--json`.
- `options payoff SYMBOL` — a strategy builder on the live chain. Legs as
  `--leg "buy call 24500" --leg "sell call 24800"` (`atm±N` strikes and
  `@PRICE` overrides too) or presets `--strategy bull-call|bear-put|straddle|
  strangle|iron-condor|butterfly|covered-call --width W`. Shows each leg's
  price (bid/ask mid, else last trade, flagged), net debit or credit, max
  profit and loss (or unlimited), breakevens, probability of profit from IV
  (lognormal), position greeks, and a braille P/L chart at expiry with an
  optional `--today` curve (Black-Scholes). Sized by `--lots` and the lot
  size; `--json` for scripts. Read-only — it never places orders.
- Technical alerts: `alert add SYMBOL rsi-below|rsi-above LEVEL [--period N]`,
  `cross-above-sma|cross-below-sma|cross-above-ema|cross-below-ema DAYS`,
  `breakout-high|breakdown-low DAYS`, `change-above|change-below PCT`,
  `volume-spike MULTIPLE [--period N]` and `results-in DAYS`. They are
  evaluated on daily bars (one chart request per symbol per check, updated
  with the live quote) by `alert check`, `alert watch`, `dashboard`, the
  `tui`, `serve` (`/api/alerts`) and `daemon longterm`; crosses fire only on
  the crossing session, and `alert watch` notifies once per trading day.
  `alert list` shows each condition in words. See
  [docs/commands.md](docs/commands.md#alert-conditions).
- `stockwise calendar [SYMBOLS] [--days 30] [--holdings|--watchlist]` —
  upcoming results, ex-dividend and payment dates, splits and bonus issues
  for holdings and the watchlist, sorted by date with today and the next
  7 days marked; `--json` / `--csv`. Yahoo Finance supplies earnings and
  dividend dates for US and Indian stocks; for `.NS` / `.BO` stocks NSE board
  meetings and corporate actions add filed results dates, splits and bonus
  issues (one polite session per run, cached for a day, Yahoo-only when NSE
  refuses).
- Data providers that read the exchanges' own websites, no API key needed:
  `STOCKWISE_DATA_PROVIDER=nse` (or `data_provider = "nse"`), `bse`, and
  `exchange` (NSE for `.NS` and NSE indices, BSE for `.BO`, Yahoo for the
  rest). NSE serves quotes with circuit limits and delivery %, NSE index
  quotes, today's intraday bars, up to five years of daily bars and search;
  BSE serves quotes, intraday and daily bars and search by scrip id, code or
  name. These are unofficial website endpoints: StockWise keeps one session per
  site, at most three requests a second with jitter, retries timeouts and 5xx
  with backoff, caches answers, and stops asking (with one notice) as soon as a
  site refuses — Yahoo answers instead. See
  [docs/data-providers.md](docs/data-providers.md#nse-and-bse) and
  [docs/exchange-responses.md](docs/exchange-responses.md).
- `quote` shows **Circuit Limits** and **Delivery** when NSE answered;
  `--json` adds `deliveryPercent`, `lowerCircuit`, `upperCircuit`, `--csv`
  adds `delivery_percent`, `lower_circuit`, `upper_circuit` (empty otherwise).
- `movers`, `heatmap` and `screen` take `--universe nifty50|niftynext50|nifty100|banknifty|niftyit`
  to scan an NSE index's current members (from NSE, whichever provider is set)
  instead of the built-in 30 large caps, which stay as the offline fallback.
- `doctor` checks NSE and BSE separately in `exchange` mode.
- Global exchanges from one market table: `-m uk`, `de`, `jp`, `hk`, `sg`,
  `ca`, `au` resolve bare tickers with the exchange's Yahoo suffix (`-m uk
  VOD` → `VOD.L`, `-m jp 7203` → `7203.T`, `-m hk 5` → `0005.HK`). Each
  exchange brings its currency (London prices in pence print as `118.20p`;
  `£`, `€`, `¥`, `HK$`, `S$`, `C$`, `A$`), benchmark indices, sector indices or
  ETFs where Yahoo has them (Japan, Hong Kong, Canada, Australia), a large-cap
  universe for `movers`/`screen`, trading hours with lunch breaks and half
  days, and 2026–27 holiday tables from the exchanges' own calendars (weekday
  fallback with a warning for years not bundled). `tax` names listings it has
  no rules for (rules exist for India, the US and the UK). See
  [docs/markets.md](docs/markets.md).
- `markets --global`: one row per exchange's main index — NIFTY 50, S&P 500,
  FTSE 100, DAX, Nikkei 225, Hang Seng, Straits Times, S&P/TSX, S&P/ASX 200 —
  with whether it is open now and its local time.
- `markets` shows the India 10-year G-sec yield (CCIL, RBI fallback; `IN10Y`
  in `--json`/`--csv`).
- `stockwise commodity [spot|futures|etfs|sgb]` and `-m commodity`: gold and
  silver in ₹ per g / 10 g / kg and USD per ounce (COMEX front month at
  USD/INR, before import duty and GST — clearly noted), COMEX futures with the
  front contract month (MCX has no free feed), NSE gold and silver ETFs, and
  Sovereign Gold Bonds with price, premium to gold spot, 2.5% interest yield
  and maturity from NSE's SGB list — or, when NSE refuses, the outstanding
  tranches without prices. With `-m commodity`, `GOLD`/`SILVER` resolve to
  `GC=F`/`SI=F` for `history`, `chart`, `technical` and alerts. `--json`/`--csv`.
- `stockwise bonds [yields|list|calc]`: T-bill, G-sec and SDL benchmark yields
  from CCIL's tenor-wise indicative yields (RBI's benchmarks as fallback;
  cached 6 hours), RBI's list of outstanding Government of India securities,
  and a calculator for price ↔ yield to maturity, accrued interest, dirty
  price, duration and T-bill discount yields, looking securities up by NSE
  symbol (`718GS2033`), name or ISIN. Individual bond prices are not shown:
  no public source publishes them. See
  [docs/commands.md](docs/commands.md#commodities--bonds).
- **WASM strategy plugins** (experimental, cargo feature `wasm-plugins`, off by
  default): `backtest SYMBOL -s wasm:path/to/strategy.wasm` runs a
  WebAssembly strategy in single, portfolio and walk-forward backtests, with
  `-P name=value` and grid tuning, and `intraday -s wasm:...` uses it as an
  extra vote. A plugin exports a small versioned ABI (`stockwise_abi_version`,
  `stockwise_on_bar`, optional `stockwise_meta` / `stockwise_init`) and runs
  in the wasmi interpreter with no imports (no WASI, files or network), a
  64 MB memory cap and a fuel budget per call: a trap, an endless loop or bad
  output is an error naming the bar, never a hang or a crash.
  `backtest strategies` lists `.wasm` plugins in the strategies folder with
  their name and ABI version. Example plugin (RSI mean reversion, identical
  to the built-in `rsi`) in `examples/wasm-strategy-rsi/`; ABI and limits in
  docs/backtesting.md ("WASM plugins").

### Changed
- Minimum supported Rust version is now **1.97** (was 1.88). StockWise
  supports the latest stable Rust and the two releases before it. Prebuilt
  binaries, Docker and Nix are unaffected.
- `alert list --json/--csv` adds `period` and `description`; `alert check
  --json/--csv` adds `period`, `description`, `value`, `reference` and the
  status `no_data` (new CSV columns come after the old ones). Existing
  `alerts.json` files load unchanged. Quotes carry Yahoo's
  `regularMarketTime`.
- In the Indian market a bare BSE scrip code resolves to BSE: `quote 500325`
  is `500325.BO` (it used to become the invalid `500325.NS`).

### Security
- `serve` sends a strict Content-Security-Policy (no inline code, same-origin
  only, Trusted Types), `X-Frame-Options: DENY`, `Referrer-Policy:
  no-referrer` and same-origin COOP/CORP headers on every response, and
  answers 403 to `/api/*` and `/metrics` requests a browser marks as
  cross-origin (`Sec-Fetch-Site`). There is still no CORS. With
  `STOCKWISE_API_TOKEN` set, the dashboard asks for the token and keeps it in
  session storage, sending it only as an `Authorization` header.

## [0.4.0] - 2026-10-21

Not released separately: these changes first shipped in 0.5.0.

Everything on the 0.3 roadmap: new markets and asset classes, a lot-based
portfolio with Indian tax rules, more brokers, a backtesting engine, a
terminal dashboard and a local API, packaging for every platform, and
Hindi. Command-line forms from 0.3 keep working.

### Added
- `stockwise tui` — a full-screen, read-only live dashboard: indices,
  watchlist, portfolio P&L, triggered alerts and a chart of the selected
  symbol (periods 1d–5y), with symbol search to add to the watchlist. Refresh
  interval with `-i` (default 30s); works at 80×24 and degrades on smaller
  terminals; honours `NO_COLOR`. See [docs/tui.md](docs/tui.md).
- `stockwise serve` — a read-only JSON API (`/api/quote`, `/api/history/{symbol}`,
  `/api/markets`, `/api/search`, `/api/portfolio`, `/api/watchlist`,
  `/api/alerts`), `/healthz`, and Prometheus `/metrics` (request counts and
  latency, Yahoo errors, cache hits, and the intraday daemon's open paper
  positions, day P&L and kill switch). Loopback only unless `--allow-remote`
  with a `STOCKWISE_API_TOKEN` bearer token. See [docs/server.md](docs/server.md).
- Cargo features `tui` and `server` (both default); `--no-default-features`
  builds without ratatui and axum.
- `--json` on 17 more commands: `portfolio`, `watch`,
  `alert list`/`check`, `technical`, `risk`, `backtest`, `screen`, `movers`,
  `sectors`, `compare`, `correlate`, `sip`, `whatif`, `returns`, `dividends`,
  `tax` and `sim history`, alongside `quote`, `history`, `markets` and
  `search`. Keys are snake_case; shapes are documented in docs/commands.md.
- `--csv` (RFC 4180, header row first) for tables: `portfolio`, `watch`,
  `alert list`/`check`, `movers`, `screen`, `sectors`, `history` bars,
  `backtest` trades, `compare`, `correlate`, `returns`, `tax`, `quote`,
  `markets` and `search`. `--json` and `--csv` are mutually exclusive.
- Config file (`config.toml` in the platform config directory, or
  `STOCKWISE_CONFIG`) for `market`, `ai`, `model`, `ollama_host`,
  `redis_url`, `home`, and a default `-p` per command under `[periods]`.
  Precedence: flag > environment variable > config file > default, and
  `--help` shows defaults from the file.
- `stockwise config [show]`, `config path` and `config init [--force]`.
- `--no-ai` turns the AI section off when `STOCKWISE_AI` or the config file
  turn it on.
- `doctor` reports the config file and whether it parsed.
- **More brokers (experimental — untested against a live account):** Zerodha
  Kite Connect, Upstox, Dhan and Fyers via `trade --broker <name>` (or
  `STOCKWISE_BROKER`): holdings (synced into `portfolio`), positions, order
  book, and orders. Built from each broker's public API docs and tested against
  a local mock server. They stay read-only until you add
  `"enable_orders": true` to the broker's config file. `trade login` handles the
  daily browser login (Kite, Upstox, Fyers) or a pasted Dhan token.
- **Stop-loss and GTT orders:** `trade buy|sell ... --trigger T [--limit P]`
  places SL-M / SL orders; `trade gtt buy|sell|list|cancel` manages single-leg
  GTTs (Angel One, Kite, Dhan Forever Orders, Fyers). Triggers are checked
  against the side, the limit and the last price before anything is sent.
  These are experimental on Angel One too and need `"enable_orders": true`.
- **OS keyring for broker secrets** (macOS Keychain, Windows Credential
  Manager, Secret Service). Secrets in an existing `angel_config.json` move
  into the keyring automatically; `trade config --keyring` does it now. Without
  a keyring (headless Linux) the owner-only file is used; `STOCKWISE_KEYRING=off`
  forces that. `doctor` shows where each broker's secrets live.
- `import groww`, `import upstox`, and `import cas` (CAMS / KFintech / NSDL /
  CDSL statements as casparser JSON or CSV). Mutual funds are stored as
  `MF:<AMFI code>`. Groww's ISIN-only exports are mapped to NSE tickers.
- **Trading costs in backtests**, on by default: `--costs in-delivery`
  (brokerage, STT, exchange and SEBI fees, GST, stamp duty, DP charge),
  `in-intraday`, `us`, `crypto`, `fx`, or `none`, plus `--slippage-bps` and
  `--brokerage`.
  Reports show net and gross returns and the money paid in costs.
- `backtest portfolio SYMBOLS...` — one strategy across several stocks with
  `--weights equal|inverse-vol|0.5,0.3,...` and `--rebalance monthly|quarterly`;
  combined equity curve, CAGR, drawdown, Sharpe/Sortino, exposure and
  per-symbol contribution.
- `backtest walkforward SYMBOL --train 1y --test 3mo` — tunes parameters on
  rolling train windows, reports only out-of-sample results, and flags
  likely overfitting.
- **Strategy files**: entry/exit rules in TOML over the built-in indicators
  (`and`/`or`/`not`, crossovers, arithmetic, stop-loss and take-profit,
  parameters and a walk-forward grid). Load with `-s NAME` from a
  `strategies/` folder next to the config file, or `-s file:PATH`; override parameters
  with `-P name=value`. The six built-ins ship as files in
  `examples/strategies/`, with two more examples.
- `backtest strategies` lists and checks strategy files.
- `intraday -s STRATEGY` adds a strategy file as an extra signal.
- `--json` on every form of `backtest` (and `--csv` for their trades); the
  `backtest --json` shape changed with the new engine (see
  docs/backtesting.md).
- Docs: [docs/backtesting.md](docs/backtesting.md).
- `stockwise man` prints the `stockwise.1` man page; `stockwise man --dir DIR`
  writes it plus one `stockwise-<command>.1` page per command. The pages are
  generated from the CLI definition and ship in every release archive under
  `man/`.
- Docker image `ghcr.io/iamsaquib8/stockwise` for linux/amd64 and linux/arm64,
  published on every release. It runs as a non-root user and keeps data in the
  `/data` volume.
- Nix flake: `nix run github:iamsaquib8/stockwise`, `nix profile install`, and
  `nix develop` for a dev shell. Installs man pages and shell completions.
- Package manifests in `packaging/` for Homebrew, Scoop, winget and the AUR
  (`stockwise-bin`), filled in from each release's checksums by
  `packaging/render.sh`. Releases push the formula to the
  `iamsaquib8/homebrew-tap` tap when a `HOMEBREW_TAP_TOKEN` secret is set, and
  attach `stockwise.json` so `scoop install <release URL>` works.
- `mf` — Indian mutual funds from AMFI's public NAV file: `mf search`, `mf nav`,
  `mf history` (NAV chart, 1M–10Y trailing returns, since-inception CAGR, max
  drawdown) and `mf sip` (monthly SIP with XIRR on the actual purchase dates).
  NAV history comes from api.mfapi.in; both are cached in the data directory
  for 12 hours. `--json` on every subcommand.
- `mf screen` ranks a category's growth-option schemes by 1Y/3Y/5Y return
  (bare `mf screen` lists categories). Expense ratios are not in AMFI's feed,
  so they are not shown for Indian schemes.
- `mf overlap A B` — shared top holdings, holdings and sector overlap, and
  expense ratios for funds Yahoo Finance has holdings for (mainly US ETFs and
  mutual funds). Indian schemes and NSE ETFs report "no holdings data".
- `options chain` (OI, change in OI, volume, IV, delta, gamma, theta, vega,
  rho), `options pcr` (put/call ratio and open-interest walls) and
  `options maxpain`. Indian symbols use NSE's option chain (index options and
  F&O stocks), US symbols (`-m us`) use Yahoo Finance. Black-Scholes greeks
  with an implied-volatility solver, valued at the data timestamp.
- `stockwise alert watch [--interval 60] [--once]` — checks alerts while
  each alert's market is open and announces every trigger once (re-arms when
  the price moves back, at most once a day; remembered across restarts).
- Notifications for `alert watch` and the intraday daemon: Telegram, Slack
  and Discord webhooks, email over SMTP (rustls), and desktop pop-ups with
  the `desktop-notify` build feature. Configured with `STOCKWISE_*`
  environment variables; `stockwise notify` shows them and `stockwise notify
  test` sends a test message. Secrets are never printed or sent anywhere but
  their own endpoint.
- Exchange clock: IST for NSE and US Eastern (with daylight saving) for
  NYSE, with the NSE 2026 and NYSE 2026–2028 holiday calendars and NYSE
  early closes.
- `sim settle --now` settles today's session before the close.
- **Crypto and forex markets.** `-m crypto` resolves bare coins (`BTC` →
  `BTC-USD`; `--quote-currency inr` / `STOCKWISE_QUOTE_CURRENCY` for `BTC-INR`)
  and `-m fx` resolves pairs (`USDINR`, `usd/inr` → `USDINR=X`). Prices show
  4 decimals for FX and up to 8 for low-priced coins; `quote` notes 24/7
  (crypto) and weekday-only (FX) trading. `markets --crypto --fx` adds top
  coins and major pairs. Session-, sector- and fundamentals-based commands
  (`sectors`, `screen`, `intraday`, `daemon`, ...) explain that they need a
  stock market instead of running on coins.
- **Data providers.** `STOCKWISE_DATA_PROVIDER=alphavantage|massive|twelvedata`
  with the provider's API key routes quotes, daily history and search through
  Alpha Vantage, Massive (formerly Polygon.io) or Twelve Data. Yahoo fills the
  fields and symbols a provider lacks and takes over on errors or exhausted
  free tiers. `doctor` shows which provider answered; `quote` names it, and
  `quote --json` adds `dataSource`. See docs/data-providers.md.
- **Cloud AI, opt-in.** `STOCKWISE_AI_PROVIDER=openai|anthropic|gemini` with
  `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `GEMINI_API_KEY` uses a hosted model
  for `report` and `--ai` (same prompts as Ollama, which stays the default).
  The first request of a run prints a notice that data is being sent;
  `doctor` shows provider and model, never the key. `STOCKWISE_MODEL`
  overrides the default model (`gpt-6.1-sol`, `claude-opus-5-5`,
  `gemini-3.8-flash`). See docs/ai.md.
- **Hindi (partial).** `--lang hi`, `STOCKWISE_LANG=hi`, or a `hi_*` locale
  translates help group titles, the main command descriptions, `quote`,
  `markets`, `dashboard`, `portfolio`, `search`, `doctor` and common errors.
  Lakh/crore amounts read `₹16.27 लाख करोड़`. `--json` output is unchanged.
- `doctor` shows the active language and where it came from.
- **Lot-level holdings.** Every purchase is a lot with its own quantity, price,
  date and source. `portfolio add ... --date YYYY-MM-DD` records the real
  purchase date; `portfolio lots SYMBOL` shows a holding's lots, sales and
  dividends, and `--lot N --date` corrects a purchase date.
- `portfolio sell SYMBOL SHARES [PRICE] [--date]` sells oldest lots first
  (FIFO) and keeps the realized P&L for `tax`.
- Portfolio XIRR: `portfolio` shows a money-weighted annual return per holding
  and for the whole portfolio (including closed positions and dividends).
- `portfolio actions [SYMBOL] [--apply]` lists splits, bonus issues and
  dividends since you bought (from Yahoo) and, with `--apply`, adjusts lots and
  records dividends. Idempotent; nothing changes without `--apply`. `divcal`
  shows the dividends received.
- Named portfolios: global `--portfolio NAME` / `STOCKWISE_PORTFOLIO`, stored as
  `portfolios/NAME.json` with their own wealth history; `portfolio list`.
- Mixed-currency portfolios: INR and USD holdings are totalled in one currency
  at the live `USDINR=X` rate (`portfolio --base inr|usd`; default: the
  market's currency). Also in `dashboard`, `wealth`, `rebalance` and the
  long-term daemon.
- `import` reads a purchase/trade date column and a buy/sell column, so a lot
  list or a tradebook (e.g. Zerodha Console) imports with real dates, realized
  sales and splits applied as they happened. A holding that already has lots
  from elsewhere is only rebuilt with `import --replace`.

### Changed
- `backtest` with too little history is an error (exit 1) instead of a
  message with exit 0.
- `movers` leaves out stocks without a change today instead of ranking them
  at 0%; `correlate` shows N/A instead of 0 when two series can't be compared.
- `watch` lists symbols that have no live quote (as N/A) instead of hiding
  them; `dividends` errors when there is no live price instead of dividing by
  zero.
- An invalid config file stops every command except `config`, `doctor` and
  `completions`, with an error naming the file and the key.
- `quote`, `history` and `markets` gather their data through a shared layer
  (`src/views.rs`) that the TUI and the server also use, so `--json` and the
  HTTP API return the same shapes.
- `portfolio --json/--csv` and `tax --json/--csv` follow the lot-based
  portfolio: holdings carry `xirr_percent`, totals are in the base currency
  (`--base`), and holdings without an exchange rate are listed in
  `unconverted`. `tax --json` reports this tax year's sales, each open
  holding's short- and long-term gain, and the estimate with and without
  selling everything today.
- The config file also covers `lang`, `quote_currency`, `portfolio`,
  `ai_provider`, `data_provider` and a `[notify]` table for notification
  channels (API keys stay in environment variables). `config show` and
  `doctor` warn when the file holds secrets and other users can read it.
- `options` is now a command group. `stockwise options SYMBOL` still shows
  expected moves, also available as `options moves SYMBOL`. `mf` and
  `options` are listed under a new "Funds & F&O" help group.
- `quote`, `markets` and `dashboard` show `N/A` for a missing price or range
  instead of `0.00`.
- AI section titles name the provider (`AI Analysis (Ollama)`) instead of
  always saying Ollama.
- EUR, GBP and JPY amounts show `€`, `£` and `¥` instead of `$`.
- `sim settle` replays each session's own 5-minute bars (falling back to
  hourly, then daily bars for old sessions): the stop or target that was hit
  first decides each trade, otherwise it exits at the 15:15 bar. It used to
  settle every session, from any date, at today's price. Today's session is
  refused until the market closes.
- `sim start` refuses to open a session when the market is closed for the
  day (weekend, holiday, or after the close).
- `daemon intraday` follows the exchange clock instead of the machine's time
  zone, skips exchange holidays, and with `-m us` trades NYSE hours.
- Intraday plans (`intraday`, `sim start`, the daemon) never need more than
  the capital in total, and the daemon's 30% per-position limit is applied.
- `tax in|us|uk` classifies each lot by its own purchase date, shows realized
  gains (sales this tax year) separately from unrealized ones, and applies the
  set-off rules: in India a short-term loss offsets STCG then LTCG, a long-term
  loss only LTCG, before the ₹1.25 lakh exemption; losses left over are shown
  as carry forward. Holdings in another currency are listed but not included.
  UK CGT uses the 24% higher rate (was 20%).
- `portfolio.json` has a new format (version 2). Older files are upgraded on
  first load — each holding becomes one lot dated when it was added, flagged as
  an estimated date — and the original is kept once as `portfolio.json.bak`.
  StockWise 0.3.0 can still read the new file.
- `import` without dates and `trade holdings` keep existing lots and only adjust
  the quantity, instead of overwriting the average cost.
- `portfolio list` now lists your portfolios; use `portfolio` or
  `portfolio show` for holdings.
- `import` exits with an error when a file has no usable rows, an unrecognized
  date, or nothing it could apply.
- `portfolio sell`, `import` and `trade holdings` refuse to match shares while
  a split or bonus isn't applied to your lots yet, and point to
  `portfolio actions --apply`.
- `wealth` snapshots are always in the market's currency.
- Every order type and broker goes through one safety flow
  (`place_live_order`): exact instrument, price checks, preview,
  `max_order_value`, `--dry-run`, typed `yes` / `--yes`. Angel One MARKET and
  LIMIT orders behave as in 0.3.
- `trade` tables show N/A instead of ₹0.00 when the broker sends no price.
- `docs/angel-one.md` is now `docs/brokers.md`.

### Fixed
- `options` (expected moves) treated a missing price as 0; it now reports an
  error.
- `daemon intraday` exited on the first network error, losing the day's
  paper positions. Errors are now logged and retried with backoff; the state
  is saved to `daemon_state.json` (`daemon_state_us.json` for `-m us`) every
  tick and resumed after a restart.
- Ctrl+C in `daemon intraday` now squares off at the last prices, records
  the day in the simulator history and exits 0, as the banner promised.
- Positions closed at the second target were not counted by the 3% daily
  loss kill switch; the kill switch also no longer re-fires every tick.
- The daemon kept waiting forever after a day with no qualified trades.
- Daemon P&L right after a first-target partial booking counted the booked
  half at the current price instead of the target price.
- Intraday plans could put a long's stop above the entry (support above the
  price) or a short's below it; such levels are now ignored.
- The daemon monitors positions from the moment it enters them, not only
  from 15 minutes after the open, and ignores zero quotes.
- Tax holding periods used the day a holding was added to StockWise, and adding
  shares kept the first purchase's date for all of them.
- Portfolio totals added INR and USD amounts together.
- `portfolio sell`, `lots`, `remove` and `actions` find a holding saved as typed
  (e.g. `AAPL` while the market is India).
- Angel One P&L and position quantities read the documented fields
  (`profitandloss`, `netqty`); they showed 0 before.
- **Backtests fill at the next bar's open** instead of the signal bar's close,
  which used a price not yet known when the signal fired. `--fill close`
  restores the old behaviour.
- Backtests trade whole shares from a starting capital (`--capital`, default
  100,000), and Sharpe/Sortino are annualised from daily returns instead of
  a per-trade ratio. Max drawdown uses the daily equity curve.
- Backtest bars with any missing price are dropped together; before, a gap in
  one series could misalign opens, highs, lows and closes.

## [0.3.0] - 2026-10-21

Usability and safety release. Command-line forms from 0.2 keep working.

### Added
- `stockwise doctor` — checks the data directory, Yahoo reachability, Redis,
  the Ollama model, and broker config in one screen.
- `stockwise completions <shell>` for bash, zsh, fish, PowerShell and elvish.
- `--json` output for `quote`, `history`, `markets` and `search`.
- Grouped top-level `--help` with examples.
- `trade buy/sell/limit --dry-run` and `--yes`; optional `max_order_value` cap.
- Environment variables: `STOCKWISE_MARKET`, `STOCKWISE_HOME`, `STOCKWISE_AI`,
  `STOCKWISE_REDIS_URL`, and the standard `OLLAMA_HOST`.
- `install.sh` one-line installer, `cargo binstall` support, SHA-256 checksums
  and a Linux ARM64 binary on every release.
- Dashboard first-run hints when you have no portfolio, watchlist or alerts.

### Changed
- **Live orders need confirmation.** Every order resolves the exact NSE `-EQ`
  instrument, prints a preview, and requires typing `yes` (or `--yes`).
- **Local AI is opt-in** (`--ai` / `STOCKWISE_AI=1`). It made analysis commands
  take 40–60 seconds. `report` still always uses AI.
- Actions are real subcommands and value lists (`portfolio`, `watch`, `alert`,
  `sim`, `trade`, `screen`, `backtest -s`, `tax`, `import`, `export`,
  `daemon`): typos are rejected with suggestions, and help documents every
  argument. `portfolio`, `watch` and `alert` show their list when run bare.
- Errors go to stderr with exit code 1 (unknown symbol, no data, bad file);
  invalid arguments exit 2. Previously many printed a message and exited 0.
- Not-found errors name the symbol and suggest `-m us` for US tickers.
- `forecast` and `stoploss` accept `-p` like the other commands.
- `sim reset` asks for confirmation and keeps a timestamped backup.
- Local data files are written atomically, and parse errors name the file.
- Redis has 300/500 ms timeouts and no longer prints a banner on every run.

### Fixed
- `M&M` and other symbols/searches containing `&` were cut off (`M&M` showed
  Macy's); all request URLs are now percent-encoded.
- `risk`, `forecast`, `options` and `deep` annualized weekly or monthly bars as
  if they were daily (volatility and Sharpe about 2.2x too high with `-p 1y`).
- SIP "annualized" return was total return divided by years; it is now XIRR.
- A missing quote was valued at 0: portfolio showed -100%, `tax` booked a full
  loss, `harvest` suggested selling, `wealth` saved a fake crash, and alerts
  fired. These holdings now show N/A and are left out.
- Re-running `import` or `trade holdings` doubled positions and reset dates;
  broker symbols like `RELIANCE-EQ` became `RELIANCE-EQ.NS`.
- Angel One credentials were saved world-readable; they are now owner-only
  (0600) and existing files are tightened. `trade setup` no longer suggests
  passing secrets as command-line arguments.
- Broker holdings/positions/order-book failures showed "No holdings found"
  instead of an error; broker calls now have timeouts.
- Index, FX and crypto symbols (`^NSEI`, `USDINR=X`, `BTC-USD`) got `.NS`
  appended in the Indian market.
- The Yahoo session token is validated; a rate-limit page used to be stored
  as the token and break every later request. Retries honour `Retry-After`.
- Volume bars drew the same glyph on every row instead of stacking.
- `--help` said the default market was US (it is India).
- `screen bluechip` used a $100B threshold for INR market caps.
- `dashboard` labelled indices by position, not symbol.
- `tax` printed a fixed "FY 2025-26"; it now shows the current tax year.
- `TATAMOTORS.NS` (no longer quoted) replaced with `TMPV.NS` in the universe.
- Piping output into `head` no longer panics.

## [0.2.0] - 2026-06-24

First public release — published to crates.io with prebuilt binaries.

### Added
- **Packaging & distribution**: published to crates.io (`cargo install stockwise`),
  prebuilt binaries for macOS (x86_64/arm64), Linux, and Windows on each release.
- **CI**: GitHub Actions running `rustfmt`, `clippy -D warnings`, and the full test
  suite (213 tests) on Linux/macOS/Windows plus an MSRV (1.88) job.
- **Project docs**: `LICENSE` (MIT), `DISCLAIMER.md`, `ROADMAP.md`, `CHANGELOG.md`,
  `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, and issue/PR templates.
- Multi-source news collation for `news`, `insider`, `ipo`, and `sentiment`.

### Changed
- Migrated to Rust edition 2024; MSRV is now 1.88 (the codebase uses let-chains).
- Codebase is `rustfmt`- and `clippy`-clean (enforced in CI).
- `risk` now defaults to a `1y` period so it has enough data to run.
- Backtest Sharpe is now a per-trade ratio (no longer annualized from a
  trade-level equity curve, which produced meaningless values).

### Fixed
- Corrected README counts (57 commands, 213 tests; was "89 tests / 50+").
- **Short trades**: `sim` status/settle and the daemon now compute P&L and
  WIN/STOP/TARGET verdicts correctly for SELL positions (previously inverted).
- **Tax**: LTCG/STCG classification now compares real dates instead of
  year+month, so the day-of-month no longer flips ~11-month holds to long-term.
- **Daemon risk**: realized P&L from the booked half at T1 is now tracked, so
  the daily-loss kill switch and EOD totals are no longer understated; wind-down
  stop tightening and breakeven moves now handle short positions.
- **Panics**: `vwap`/`atr` no longer index out of bounds on ragged OHLCV data;
  every `partial_cmp().unwrap()` sort is now NaN-safe.
- **Division by zero / NaN**: guarded in `daily_returns`, `value_at_risk`,
  backtest returns, `sip`, `stoploss`, `rotation`, dividend/harvest P&L, and
  swing-point detection.
- `markets` now matches index labels to quotes by symbol (Yahoo can reorder
  results); `whatif` pairs timestamps with closes so the buy price stays aligned.
- Live order placement warns instead of going silent when no order id is
  returned, preventing accidental resubmits; `triggerprice` is only sent on
  stop-loss order types.
- `strip_think_tags` (AI output) no longer corrupts text when a stray closing
  tag precedes the opening one.

## [0.1.0] - Unreleased

Initial development. Not published. Highlights that landed during this phase:

### Added
- 57 commands spanning analysis, screening, trading bots, live trading, and
  portfolio management for US and Indian (NSE/BSE) markets.
- Daemon workers: continuous intraday bot (9 strategies, Kelly sizing, risk
  controls) and a daily long-term worker (6-pillar scoring + Monte Carlo).
- Paper-trading simulator with confidence rating.
- Live trading via Angel One; broker import for Zerodha Kite, INDmoney, and CSV.
- Local AI integration via Ollama, auto-injected into 6 commands.
- Redis response caching with in-memory fallback.
- Rate limiting, retry-with-backoff, and response caching for the Yahoo Finance
  data source.
- Multi-timeframe and braille terminal charts; `-p/--period` flag on chart commands.
- `deep` command — full report on a single stock in one shot.

[Unreleased]: https://github.com/iamsaquib8/stockwise/compare/v0.8.2...HEAD
[0.8.2]: https://github.com/iamsaquib8/stockwise/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/iamsaquib8/stockwise/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/iamsaquib8/stockwise/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/iamsaquib8/stockwise/compare/v0.5.0...v0.7.0
[0.6.0]: https://github.com/iamsaquib8/stockwise/compare/v0.5.0...d9f52f2
[0.5.0]: https://github.com/iamsaquib8/stockwise/compare/v0.3.0...v0.5.0
[0.4.0]: https://github.com/iamsaquib8/stockwise/compare/v0.3.0...c2d75af
[0.3.0]: https://github.com/iamsaquib8/stockwise/compare/5bbf8ec...v0.3.0
[0.2.0]: https://github.com/iamsaquib8/stockwise/tree/5bbf8ec
[0.1.0]: https://github.com/iamsaquib8/stockwise/commits/5bbf8ec
