# Architecture

## Source Layout

StockWise is one crate and one binary. `src/main.rs` defines the CLI; the
rest are modules it calls. The analysis engines are pure functions over plain
inputs, so they test without the network.

```
src/
  main.rs            CLI definition (clap): 95 top-level commands, the grouped help table
                     (`--help` core commands, `--help-all` every command), dispatch, parse tests
  commands/          Command handlers, one file per area; each `cmd_*` is re-exported by mod.rs
    overview.rs      quote, markets, dashboard, search, doctor, cache status / clear
    background.rs    cache warm / snapshots, worker start / stop / status / run, tasks
    service.rs       worker install / uninstall / service-status (launchd agent, systemd user unit)
    context.rs       use (default market, active portfolio)
    init.rs          init (first run)
    setup.rs         config path / show / init, profile show / set / reset
    update.rs        update (see selfupdate.rs)
    fundamentals.rs  analyze, compare, peers, dividends, insider, deep, fundamental score, results
    research.rs      events, insights, verdict (printing; data in research.rs, events.rs, verdict.rs)
    actions.rs       portfolio review; the holding context behind verdict, deep and a bare SYMBOL;
                     quick actions for the portfolio and checkup tables
    technicals.rs    technical, history, chart, timeframes, patterns, support, fibs, volume, gaps, stoploss
    risk.rs          risk, correlate, returns, forecast, options moves
    markets.rs       movers, sectors, news, sentiment, conditions, screen, picks, picks review,
                     earnings, rotation, matrix, surprise, heatmap, sectorcmp
    hot.rs           hot
    today.rs         today
    monitoring.rs    monitor, digest, and the monitor passes the worker runs
    bot.rs           telegram (two-way bot)
    statusbar.rs     statusbar
    ipo.rs           ipo, ipo review, ipo calibrate
    calendar.rs      calendar
    funds.rs         mf search, nav, history, sip, screen, overlap
    fno.rs           options chain, pcr, maxpain, oi, payoff
    commodities.rs   commodity
    bonds.rs         bonds yields, list, calc
    income.rs        income
    backtests.rs     backtest (single, portfolio, walkforward, strategies)
    strategies.rs    sip, whatif, intraday, longterm, report
    sim.rs           sim start / status / history / settle / reset
    practice.rs      sim replay, sim challenge, sim profile, sim card
    worker.rs        daemon intraday / longterm loops
    portfolio.rs     portfolio show / add / sell / lots / actions / demerger / rename /
                     return-of-capital / remove / restore / list / target / xray, and rebalance;
                     valuation in a base currency (`value_holdings`)
    portfolio_analyze.rs  portfolio analyze
    checkup.rs       portfolio checkup, portfolio wrap
    taxpreview.rs    portfolio sell --preview, mf bleed
    planning.rs      tax export, portfolio benchmark, portfolio pme, goal
    tax_season.rs    tax reconcile, tax income, tax foreign, tax export --schedule / --audit-pack, tax advance
    holdings.rs      watch, alert, import (Kite, INDmoney, generic CSV and .xlsx, tradebooks), export,
                     wealth, harvest, tax, divcal, pcorr
    imports.rs       Groww / Upstox and CAS (casparser JSON/CSV) import parsers
    casbridge.rs     import cas FILE.pdf through casparser
    alerting.rs      alert watch loop, notify status / test
    trade.rs         live orders for every broker: place_live_order (validation, preview, cap, confirmation)
    broker_selftest.rs  trade selftest (read-only checks and a redacted report)
    broker_sync.rs   trade sync, trade gtt list status changes, portfolio sync
    interfaces.rs    tui and serve (behind the `tui` / `server` cargo features)
    assistant.rs     ask, mcp, undo, connect
    output_tests.rs  --json / --csv shape tests (the shapes in docs/commands.md)
  action.rs          Rule-based actions: BUY / WATCH / AVOID, ADD / HOLD / TRIM / EXIT, NO CALL
                     (see "Actions" below)
  verdict.rs         6-pillar score from fixed rules; pillars without data are left out, `data_coverage`
  picks.rs           picks: five percentile-ranked pillars per scan, long and short horizons
  picks_log.rs       picks track record (`signal_log.json`) for picks review
  provenance.rs      Where each figure came from: sources, field coverage, warnings, the footer
  research.rs        One research snapshot per stock (Yahoo, NSE/XBRL filings, news) for events,
                     insights, verdict
  insights.rs        Rule-based plain-English insights
  events.rs          Key-events timeline: de-duplicated, categorised, with the price reaction
  scorecard.rs       Piotroski F-score and Altman Z (Z'') with every criterion and formula
  results.rs, results/  Quarterly and annual results, earnings-season scoreboard
  conditions.rs      Market readings: trend, volatility, breadth, valuation
  insider.rs         Insider trades from the regulator's filings
  news.rs            Headlines and exchange filings per stock; the merged feed for holdings and watchlist
  pulse.rs           What moved today and why: shared by today, hot, the digest, the bot and MCP
  hot.rs             Heat ranking and market scanners (moves, volume, gaps, deals, FII/DII, F&O ban, OI)
  crossmarket.rs     Picks the market for a symbol when -m is not given
  monitor.rs         Holding monitor detectors and thresholds (`monitor_state.json`)
  digest.rs          The after-close digest as one message; alert log
  notify.rs          Notifications: Telegram, Slack, Discord, SMTP email, ntfy, Pushover, desktop;
                     priorities, routes, quiet hours
  alerts.rs          Alert conditions and their evaluation on quotes and daily bars
  calendar.rs        Results, dividend and split dates (Yahoo + NSE), day cache
  portfolio.rs       Lots, FIFO sales, splits/dividends, XIRR cash flows, FX, named portfolios,
                     import rules, the corporate-action journal
  corp_actions.rs    Splits and bonuses not yet applied to lots; broker holdings drift
  trade_charges.rs   portfolio add/sell --charges (auto estimate for NSE/BSE delivery trades)
  sellpreview.rs     FIFO lots, tax and LTCG countdown for a sale that is not recorded
  portfolio_analysis.rs  Figures behind portfolio analyze
  checkup.rs, wrap.rs    portfolio checkup lenses; portfolio wrap year in review
  xray.rs            Look-through overlap, asset-class, market-cap and sector split
  index_weights.rs   Shipped index weights for ETF look-through
  business_groups.rs Indian promoter groups for concentration checks
  benchmark.rs       Time-weighted return vs an index, alpha/beta/tracking error, attribution
  pme.rs             Public Market Equivalent (Index Twin)
  rebalance.rs       Target weights, drift bands, buy-only, sell-only (`--raise`) and tax-aware plans
  income.rs          Dividend, coupon and passive-income analytics
  bleed.rs           Regular vs Direct mutual-fund cost
  goals.rs           SIP / step-up, retirement, education, SWP, prepay math; Indian amounts (1cr, 25L, 50k)
  tax.rs             Capital-gains rules: holding periods, tax years, set-off, cess, sale-date rates,
                     advance tax and Sec. 234C (India/US/UK)
  tax_export.rs      ITR Schedule CG / 112A and Form 8949 from realized sales
  tax_reconcile.rs   Broker tax P&L parser (Zerodha, Groww, Upstox) and sale-by-sale matcher
  tax_income.rs      Dividends and interest with TDS (Schedule OS)
  tax_foreign.rs     Schedule FA and Form 67
  fx_history.rs      Historical exchange rates for costing foreign purchases
  pta_export.rs      portfolio export as a beancount or hledger journal
  backup.rs          backup export / restore (passphrase-encrypted .swbak)
  selfupdate.rs      update: how the binary was installed, newer release, in-place replace
  api.rs             Yahoo Finance client (session, quotes, charts, search), cache, rate limit,
                     retries; NSE requests share one session; routes to a provider
  providers/         MarketData trait and routing (router.rs) over the configured providers:
                     nse.rs, bse.rs, bse_bhav.rs, niftyindices.rs (exchange sites), edgar.rs and edgar/
                     (SEC filings), alphavantage.rs, massive.rs, twelvedata.rs, finnhub.rs, fmp.rs
                     (keyed), coingecko.rs, frankfurter.rs (keyless backups), fill.rs (fill missing
                     fundamentals field by field), adjust.rs (split adjustment), session.rs (polite
                     exchange HTTP: one cookie session per site, rate limit, a refusal stops)
  db.rs              cache.db: SQLite (WAL) with http_cache, market snapshots, bars, tasks;
                     safe to delete
  pagecache.rs       Cached fetch of public pages (CCIL, RBI, NSE SGB list) with a stale-copy fallback
  nifty500.rs, sp500.rs, global_lists.rs  Shipped index member lists, the offline scan fallback
  market.rs          Market enum and MarketInfo table, symbol resolution, index/sector/coin/pair lists
  clock.rs           Exchange clocks, holidays, half days, lunch breaks, session phases
  funds.rs           AMFI NAVs and TER, api.mfapi.in history, returns, SIP, ETF overlap
  offmarket.rs       Prices Yahoo doesn't give: MF: funds at AMFI's NAV, SGBs; ETF categories
  derivatives.rs     Option chains (NSE / BSE / Yahoo), lot sizes, PCR, max pain, IV and greeks
  greeks.rs, payoff.rs  Black-Scholes and IV solver; multi-leg payoffs
  commodity.rs       Gold and silver spot in rupees, futures, ETFs, SGBs
  bonds.rs, gsec.rs  Bond math; CCIL yields, RBI benchmarks, G-sec list
  ipo.rs             Indian IPO board, subscription, allotment odds, regime; assembles the verdict
  ipo_verdict.rs     IPO verdicts (APPLY / WAIT / AVOID / NO CALL): valuation, odds, EV, gates, plan
  ipo_model.rs       Listing-day model: shipped fit, Huber regression, residual P(loss), regime
  ipo_log.rs         IPO verdict log (`ipo_signal_log.json`) and `ipo review`
  technical.rs       Indicators and statistics (SMA, EMA, RSI, MACD, BB, ATR, VWAP, OBV, VaR, Sharpe, XIRR)
  backtest/          Backtesting engine (docs/backtesting.md): rules.rs, indicators.rs, costs.rs,
                     engine.rs, walkforward.rs, stats.rs, regression.rs, plugin.rs (WASM, `wasm-plugins`)
  intraday.rs, longterm.rs  Intraday signal scanner and sizing; long-term scores and Monte Carlo
  daemon.rs, daemon_status.rs  Paper-trading daemon state; its status for /metrics
  simulator.rs       Paper-trading sessions, costs, track record
  practice/          Historical replays, challenges, scores, badges, cards (docs/simulator.md)
  worker.rs          Background worker: market-aware ticker, task executor, end-of-day sync
  views.rs           Data behind quote/history/markets/portfolio/watchlist/alerts as structs, shared by
                     the CLI, the TUI and the server; the `DataSource` trait
  tui/               stockwise tui: app.rs (state, keys), ui.rs (drawing), analysis.rs, mod.rs (event loop)
  server/            stockwise serve: mod.rs (axum routes, bearer auth), metrics.rs (Prometheus),
                     web.rs (embedded dashboard, CSP headers)
  mcp.rs, mcp/       stockwise mcp: tools, resources, prompts, stdio and Streamable HTTP; mcp/tests.rs
  tools/             Tools shared by mcp and ask (see "MCP and AI" below)
  ask.rs, ask/       stockwise ask: the tool-calling loop per provider, sessions, playbooks
  ai.rs              AI providers (Ollama, OpenAI, Anthropic, Gemini), prompts, the explain-only rules
  connect.rs         stockwise connect: each AI app's MCP config, merged in place with a backup
  audit.rs           Journal of assistant changes for undo
  onboarding.rs, notes.rs  Profile questions; per-stock thesis and limits (notes.json)
  brokers/           Broker trait and clients: angel, zerodha, upstox, dhan, fyers (experimental);
                     secrets.rs (OS keyring), readonly.rs, mock.rs, gtt_watch.rs, charges.rs,
                     masters.rs, app.rs, http.rs
  config.rs          Optional config.toml: defaults, [profile], [monitor], notify settings
  store.rs           Atomic JSON persistence, data directory (STOCKWISE_HOME), file locks
  output.rs          --json / --csv rendering (Format, CsvRow, RFC 4180 quoting)
  charts.rs, display.rs  Braille charts and bars; currency- and asset-aware formatting
  i18n.rs            English/Hindi message catalog
  xlsx.rs            Minimal .xlsx reader
  watchlist.rs, wealth.rs  Data models persisted via store.rs
  testutil.rs        Mock HTTP server for tests
  fixtures/          Parser fixtures (exchange pages, XBRL, broker tax P&L files)
```

Adding a command: define it in `Commands` in `main.rs` (and in the `GROUPS`
help table; a test enforces this), write `cmd_<name>` in the matching
`commands/*.rs` file, dispatch to it in `main()`, and list it in
`docs/commands.md`. For machine-readable output, flatten `TableOutput`
(`--json`/`--csv`) or `JsonOutput` (`--json`) into the command, build the rows
once as `Serialize` structs, and branch on `output::Format`; shape tests live in
`commands/output_tests.rs`.

## Configuration

`main()` loads the config file before parsing arguments (`config::load`) and
turns its `market`, `ai` and `[periods]` values into clap defaults, so flags and
env vars still win and `--help` shows the effective default. Other settings
are read through `config::setting(env_var, ...)`: env var first, then the file,
then the caller's built-in default. Precedence is always flag > env var >
config file > default.

## Data Flow

Every command follows the same path: providers fetch, `api.rs` caches, a
command handler computes, and `output.rs` or a printer renders.

```
providers (Yahoo, NSE/BSE, EDGAR, AMFI, CCIL/RBI, keyed or keyless backups)
   │  providers/router.rs picks a source per capability; fill.rs fills missing fields
   ▼
api.rs  ── rate limit (5 req/s), retries, one shared NSE session
   │  cache: in-memory → Redis (if set) or cache.db → network
   ▼
engines (pure): technical, verdict, action, picks, tax, rebalance, portfolio, ...
   │
   ▼
commands/*  (CLI)   tui/   server/   tools/ (mcp, ask)
   │  views.rs gives the CLI, TUI and server the same structs
   ▼
output: table (display.rs, charts.rs) │ --json / --csv (output.rs) │ HTTP JSON │ MCP structuredContent
```

User data (portfolio, watchlist, alerts, goals, notes, config) is read and
written through `store.rs`; it never goes into a cache. Live orders go only
through `place_live_order` in `commands/trade.rs`.

`--verbose` (or `STOCKWISE_LOG=debug|info|warn`) prints each request's host,
status, time and whether the cache answered on stderr. See
[troubleshooting.md](troubleshooting.md).

## Actions

`src/action.rs` is the only place that computes an action. `decide(&Inputs)`
is pure and deterministic: the same inputs give the same action. Builders fill
`Inputs` from a research snapshot and its verdict
(`inputs_from_research`) or from a quote and its long-term score
(`inputs_from_quote`), then add the investor profile and, for a holding, the
holding context.

- A stock you don't hold gets BUY, WATCH or AVOID; one you hold gets ADD, HOLD,
  TRIM or EXIT.
- Data coverage below `MIN_COVERAGE` (60%) gives NO CALL with the missing
  fields.
- Every action carries at most three reasons, a confidence level, a review
  date, the methodology version (`METHODOLOGY`) and the disclaimer; BUY, ADD,
  HOLD and TRIM also carry an invalidation price.
- `commands/actions.rs` connects it to `portfolio review`, `verdict`, `deep`,
  the portfolio and checkup tables; `picks.rs` adds an action to each finalist;
  `pulse.rs` adds one to `today`, `hot` and the digest; the MCP tools
  `stock_action` and `portfolio_review` return it.
- AI explains the computed action (`ai.rs` rules, MCP instructions). It never
  makes one up or changes it.

The rules and thresholds are in [methodology.md](methodology.md#actions-methodology-1).
A change to a rule bumps `METHODOLOGY`. Tests that print an action use
`crate::action::testing::assert_action_well_formed`.

## MCP and AI

- `src/tools/` holds the tools that `stockwise mcp` and `stockwise ask` share.
  `mod.rs` defines them (`TOOLS`: names, typed parameters, hints, timeouts) and
  the result envelope; `handlers.rs`, `extra.rs`, `more.rs` and `pulse.rs` run
  the read tools, each calling the function behind a command's `--json`;
  `schema.rs` has the output schemas; `workspace.rs` has the opt-in write tools,
  each recorded in the audit journal (`audit.rs`) so `stockwise undo` can
  reverse it; `args.rs` checks arguments. `stockwise mcp --list-tools` lists 71
  tools.
- `src/mcp.rs` is the MCP server (rmcp): tools, resources
  (`stockwise://...`), prompts, completions, stdio and Streamable HTTP. It is
  read-only unless started with `--allow-writes` / `--allow-portfolio`.
  `packaging/mcpb/manifest.json` lists every tool; a test compares the lists.
- `src/connect.rs` writes each AI app's MCP config (`stockwise connect`).
- `src/ask.rs` runs the tool-calling loop for `stockwise ask` in each
  provider's dialect; `src/ai.rs` holds the providers and prompts.

## Worker

`src/worker.rs` runs background work backed by `cache.db`:

- **In process.** `tui`, `serve` and `mcp` start a light ticker that keeps
  held and watched quotes warm and broadcasts changes (the server's
  `/api/events`).
- **As a service.** `stockwise worker start` (detached) or `worker run`
  (foreground) adds the task executor (`tasks add`), alert checks, the holding
  monitor during the session, and the post-close sync: the digest, notices,
  broker holdings drift and snapshots. `worker install` registers `worker run`
  as a launchd agent (macOS) or a systemd user service (Linux)
  (`commands/service.rs`). State is in `worker_state.json`; the service logs to
  `worker.log`.

The worker never places orders. A failed fetch keeps the last data and backs
off.

## Caching

Layers, checked in order:

```
Request → memory → second layer → network → store in memory and the second layer
                   (Redis when STOCKWISE_REDIS_URL is set, else cache.db http_cache)
```

`cache.db` is a SQLite file in the data directory. It holds HTTP responses,
market snapshots (`cache warm`), daily bars, results, index members and the
task queue. It only holds data that can be fetched again, so deleting it is
safe; `stockwise cache clear [http|quotes|snapshots|bars|results|members|bhavcopy|all]`
removes parts of it, and `stockwise cache status` shows its size.

| Data | TTL | Why |
|------|-----|-----|
| Quotes | 30s | Live prices, short freshness |
| Charts (1d/5d) | 60s | Intraday data |
| Charts (1mo/3mo) | 5 min | Historical, moderate |
| Charts (1y+) | 10 min | Rarely changes |
| Search | 2 min | Static-ish |

Redis is off by default. Point at a server with `STOCKWISE_REDIS_URL` (or
`redis_url` in the config file); `off` turns it off again. Keys use the `stockwise:`
prefix. Connection and response timeouts (300/500 ms) keep a missing or hung
Redis from slowing commands.

```bash
redis-cli --scan --pattern 'stockwise:*' | xargs redis-cli del   # clear only StockWise keys
```

Rate limiting: 200 ms minimum between Yahoo requests (5 req/s). Retry: 3
attempts with exponential backoff; HTTP 429 honours `Retry-After` (capped at
10 s) and never sleeps after the last attempt. Exchange sites (NSE, BSE) get
one cookie session each, at most 3 requests a second; a refusal stops requests
to that site for a while.

## Data Storage

Location: `~/Library/Application Support/stockwise/` (macOS), `~/.local/share/stockwise/` (Linux), or
`%LOCALAPPDATA%\stockwise\` (Windows). Set `STOCKWISE_HOME` (or `home` in the config file) to use another directory;
`stockwise doctor` prints the active path. Files are written atomically (temp file + rename), and
broker config files are readable only by you (0600 on Unix). Broker secrets move into
the OS keyring when one is available (`STOCKWISE_KEYRING=off` keeps them in the file).

| File | Purpose |
|------|---------|
| `portfolio.json` | Default portfolio: holdings with lots (qty, price, purchase date, source), realized sales, dividends. Format version 2; older files are upgraded on load and kept once as `portfolio.json.bak` |
| `portfolios/NAME.json` | Named portfolios (`--portfolio NAME`), same format |
| `portfolios/NAME.wealth_history.json` | Wealth history of a named portfolio |
| `watchlist.json` | Watched symbols |
| `alerts.json` | Price alerts (symbol, condition, target) |
| `wealth_history.json` | Daily value snapshots of the default portfolio |
| `sim_history.json` | Paper trading sessions and results |
| `sim_replay.json` / `sim_replay_tape.json` | The running historical replay and its cached daily prices (removed by `sim replay end`) |
| `sim_challenge.json` | A challenge running over live paper sessions |
| `sim_profile.json` | Practice badges, streaks, best scores and recent runs |
| `daemon_state.json` / `daemon_state_us.json` | The intraday daemon's current day (positions, risk), saved every tick |
| `alert_watch.json` | Which alerts `alert watch` already announced |
| `angel_config.json` | Angel One API credentials (secrets in the OS keyring when available) |
| `zerodha_config.json`, `upstox_config.json`, `dhan_config.json`, `fyers_config.json` | Experimental broker apps and daily sessions (secrets in the keyring when available) |
| `cache.db` | SQLite cache: HTTP responses, market snapshots, bars, results, index members, background tasks, `ask` sessions (safe to delete; `cache clear`) |
| `goals.json`, `notes.json` | Saved goals; per-stock thesis and limits |
| `signal_log.json` | Every `picks` list, for `picks review` |
| `monitor_state.json`, `alert_log.json` | Monitor signals already sent; alert history for the digest |
| `worker_state.json`, `worker.log` | Background worker state and the login service's log |
| `audit.json`, `undo/` | Changes made by an assistant, for `stockwise undo` |
| `advance_tax.json` | Advance-tax payments recorded with `tax advance --record` |
| `holdings-drift.json` | Daily broker holdings drift report (`portfolio sync`) |
| `update-check.json` | When `doctor` / `update` last looked for a new release |
| `cache/` | AMFI NAV file and per-scheme NAV history, re-downloaded after 12 hours; CCIL/RBI pages (6–24 hours) and NSE's SGB list (5 minutes) (safe to delete) |

## Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing (derive macros) |
| `clap_complete` / `clap_mangen` | Shell completions and man pages generated from the CLI definition |
| `reqwest` | HTTP client (Yahoo Finance, brokers, Ollama) |
| `keyring` | OS keyring for broker secrets |
| `async-trait` | `dyn Broker` with async methods |
| `sha2` / `flate2` | Kite/Fyers login checksums; gzipped instrument lists |
| `rpassword` | Hidden prompt for pasted tokens |
| `redis` | Redis caching (optional, falls back to in-memory) |
| `tokio` | Async runtime |
| `serde` / `serde_json` | JSON serialization |
| `colored` | Terminal colors |
| `chrono` | Date handling |
| `dirs` | Platform-specific data directories |
| `anyhow` | Error handling |
| `lettre` | SMTP email notifications (rustls) |
| `notify-rust` | Desktop notifications (optional `desktop-notify` feature) |
| `ratatui` | Terminal UI for `tui` (re-exports crossterm; `tui` feature) |
| `axum` | HTTP server for `serve` (`server` feature) |
| `wasmi` | WebAssembly interpreter for strategy plugins (optional `wasm-plugins` feature) |
| `rmcp` | Official Model Context Protocol SDK for `stockwise mcp`, without its macros (`mcp` feature) |

`tui`, `server` and `mcp` are on by default (`mcp` needs `server`);
`cargo build --no-default-features` leaves them out (`ask` still works).
`wasm-plugins` is off by default. CI-style checks should run clippy for all
three builds.

## Releases and packaging

Pushing a `v*` tag runs `.github/workflows/release.yml`: it builds the six release
archives (binary, docs, and `man/*.1` from `stockwise man --dir`; Linux, macOS, Windows
and Android for Termux), the `.deb`/`.rpm` packages (`packaging/linux/build-packages.sh`,
metadata in `Cargo.toml`) and the PyPI wheels (`pyproject.toml`, maturin `bin`),
publishes the crate, renders the Homebrew, Scoop, winget and AUR manifests in
`packaging/` with the release's checksums, updates the Homebrew tap and uploads to PyPI
and Docker Hub when their credentials are configured, and pushes the multi-arch
`ghcr.io/iamsaquib8/stockwise` image built from `Dockerfile`. `flake.nix` builds the
same crate with Nix; `packaging/nixpkgs/` and `packaging/termux/` hold the nixpkgs and
termux-packages recipes. `.github/workflows/stats.yml` archives download counts nightly
(`scripts/download-stats.py`). See [packaging/README.md](../packaging/README.md).

## Testing

```bash
cargo test                           # unit tests, CLI parsing tests and tests/
cargo test technical                 # only tests whose name matches
cargo test --features wasm-plugins   # also the WASM strategy plugin tests
```

Unit tests sit next to the code they test, and the pure engines (`technical`,
`tax`, `rebalance`, `verdict`, `backtest`) are tested without the network.
Parsers of exchange and broker pages run against fixtures in `fixtures/`
directories, and the HTTP clients against `testutil::MockServer`, so no test
reaches a real service. `tests/` holds the integration tests that run the
built binary against a scratch `STOCKWISE_HOME`. Tests also read the docs:
every `stockwise ...` command in backticks in `docs/cheatsheet.md`,
`skills/stockwise/SKILL.md` and the guides (and in their `bash` blocks) must
parse, so a renamed flag breaks the build until the docs follow.

CI (`.github/workflows/ci.yml`) runs `scripts/changelog.py check`, the tests on
Linux, macOS and Windows, `cargo fmt --check`, clippy for the default and the
`--no-default-features` builds, a check on Rust 1.97, the `wasm-plugins`
feature and `shellcheck` on `install.sh`.

## Docs and changelog

- `docs/*.md` are the documentation pages. `python3 site/build.py` (needs
  [mdBook](https://rust-lang.github.io/mdBook/)) copies them with `site/index.md`
  and `site/SUMMARY.md` into `target/site-src`, builds `target/site`, and writes
  `llms.txt`, `llms-full.txt` and a Markdown copy of every page.
  `.github/workflows/docs.yml` publishes it to GitHub Pages on every push to `main`.
- `changelog.d/` holds one small file per user-visible change.
  `scripts/changelog.py` checks the files and folds them into `CHANGELOG.md` at
  release time; see [CONTRIBUTING.md](../CONTRIBUTING.md#changelog-entries).
