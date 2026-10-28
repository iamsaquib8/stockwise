# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-10-26

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
  ISIN) use AMFI's latest end-of-day NAV, shown as "NAV as of <date>"; SGBs (NSE
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

[Unreleased]: https://github.com/iamsaquib8/stockwise/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/iamsaquib8/stockwise/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/iamsaquib8/stockwise/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/iamsaquib8/stockwise/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/iamsaquib8/stockwise/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/iamsaquib8/stockwise/releases/tag/v0.2.0
[0.1.0]: https://github.com/iamsaquib8/stockwise/commits/master
