# Roadmap

Where StockWise is headed. Dates are intentionally omitted — this is a
direction, not a promise. Have an opinion? Open a
[Discussion](https://github.com/iamsaquib8/stockwise/discussions) or upvote an
[issue](https://github.com/iamsaquib8/stockwise/issues) — the roadmap follows
demand.

Legend: ✅ shipped · 🛠️ in progress · 📋 planned · 💡 exploring

## ✅ Shipped

**0.6** — AI assistants and more ways to install
- `stockwise mcp`: 24 read-only tools, resources and prompts for Claude, Cursor and other AI
  assistants (no orders); `stockwise ask` with tool calling; `llms.txt` and an Agent Skill
- Install: PyPI wheels (`pipx`/`uvx`, published once enabled), `.deb`/`.rpm`, a Termux build,
  nixpkgs and Termux recipes, a Docker Hub mirror; Linux binaries run on glibc 2.34+
  (Ubuntu 22.04, Debian 12, RHEL 9); download stats without telemetry
- Mutual funds and SGBs valued across portfolio views and tax; `MF:` symbols fixed
- Educational AI wording instead of buy/sell verdicts
- Documentation site with a cheatsheet and a comparison with other tools

**0.5** — planning, reporting and more markets
- Planning: capital-gains export (`tax export in|us`), portfolio vs benchmark with attribution,
  goal planner (`goal sip|retire|education`)
- Alerts: RSI, moving-average crosses, breakouts, day-change and volume spikes; results calendar
- Markets: UK, Germany, Japan, Hong Kong, Singapore, Canada, Australia from one table; gold and
  silver (spot ₹/10 g, ETFs, SGBs, COMEX); G-sec yields and a YTM calculator; NSE/BSE data provider
- Funds & options: AMFI expense ratios, BSE options, options strategy builder with payoff charts
- Simulator: historical replay with the future hidden, challenges, risk-adjusted scores, badges
- Brokers: read-only `trade selftest` + tester kit, `trade sync` of fills and GTT status
- Web dashboard in `stockwise serve`; experimental WASM strategy plugins; MSRV 1.97

**0.4** — the whole 0.3 roadmap
- Markets: crypto and FX (`-m crypto`, `-m fx`), mutual funds via AMFI (`mf search/nav/history/sip/screen`),
  fund/ETF overlap, F&O option chains with greeks, IV, PCR and max pain (NSE + US)
- Portfolio: lots with purchase dates, FIFO sales, XIRR, splits/bonuses/dividends, named
  portfolios, INR/USD totals, Indian LTCG/STCG with set-off rules
- Brokers: Zerodha, Upstox, Dhan, Fyers (**experimental** — untested against live accounts)
  next to Angel One; stop-loss and GTT orders; OS keyring; Groww, Upstox and CAS imports
- Bots & alerts: resilient paper daemon (Ctrl+C square-off, saved state), IST/US exchange clock
  with holidays, `sim settle` from intraday bars, `alert watch`, Telegram/Slack/Discord/email/desktop
- Backtesting: Indian/US cost models, next-bar fills, portfolio and walk-forward backtests,
  TOML strategy files usable by the intraday bot
- Interfaces: `stockwise tui` dashboard, `stockwise serve` JSON API + Prometheus metrics
- Scripting: `--json` / `--csv` across commands, `config.toml`, man pages
- Data & AI: Alpha Vantage / Massive (Polygon) / Twelve Data providers, opt-in OpenAI /
  Anthropic / Gemini, Hindi (`--lang hi`, partial)
- Distribution: Docker image, Nix flake, Homebrew/Scoop/winget/AUR manifests

**0.3** — usability and safety
- `doctor`, shell completions, grouped help, `--json` on core commands
- Typed subcommands with suggestions; real exit codes
- Live orders behind exact instrument matching, preview, and confirmation
- Opt-in local AI; owner-only credential storage; atomic data files
- One-line installer, `cargo binstall`, checksums, Linux ARM64 binaries

**0.2** — first public release
- 57 commands across analysis, screening, bots, trading, and portfolio
- Daemon workers, paper-trading simulator, Angel One live trading
- Broker import (Kite, INDmoney, CSV), Redis cache, braille charts

## 🚀 Reach — get StockWise to more people (0.6.x)

StockWise has the features; most people who would use it haven't found it yet.
This comes first.

- 🛠️ **Homebrew tap, Scoop bucket, winget and AUR** — manifests are generated on every release;
  publishing them needs the tap repository and package submissions. Then homebrew-core once
  eligible (75★ when a user submits it)
- 🛠️ **List `stockwise mcp` where people look** — the official MCP registry (`server.json` is
  ready), awesome-mcp-servers, and Claude Desktop extensions (`packaging/mcpb/`) attached to
  releases (Zerodha, Groww, Dhan, Upstox and Fyers ship MCP servers; Angel One doesn't)
- 🛠️ **`pipx install stockwise` / `uvx stockwise`** — PyPI wheels carrying the binary (maturin
  `bin`), where India's Python trading community already installs its tools. Wheels (manylinux,
  musllinux, macOS, Windows) and the sdist are built on every release; publishing needs the
  one-time PyPI trusted-publisher setup
- 📋 **Animated demo** (VHS) at the top of the README and terminal recordings in the docs
  ([good first issue #23](https://github.com/iamsaquib8/stockwise/issues/23))
- 🛠️ **More places to install** — submit the prepared nixpkgs and termux-packages recipes, and
  turn on the Docker Hub mirror of the GHCR image (needs Docker Hub credentials)
- 📋 **Try it without installing** — a Codespaces/devcontainer setup and a read-only hosted demo
  that runs on bundled historical data
- 📋 **Daily replay challenge** with a shareable image card, built on the simulator's scoring

## 🛠️ Now — loose ends (0.6.x)

- 📋 **Direct `.xlsx` imports** — teach the built-in reader dates and sheet selection, and read
  Groww, Zerodha Console and Upstox `.xlsx` holdings and tradebook exports without saving as CSV;
  add an Angel One import
- 🛠️ **Broker integrations verified against live accounts** — the read-only `trade selftest` and the
  [verification criteria](docs/brokers.md#help-verify-a-broker) are ready; we need 3 reports per
  broker plus one small live order a tester places themselves
- 📋 **SEBI retail-algo rules** (in force since 1 Apr 2026) in `trade` — order-rate limits,
  price-protected market orders, daily login and static-IP guidance
- 📋 **Check the NSE/BSE parsers against live responses** and add SENSEX from BSE (the endpoints are
  unofficial; licensed exchange feeds stay out of scope)

## 📋 Next — your money, end to end, and more markets (0.7)

**Tax season** (ship before the 15 Dec and 15 Mar advance-tax dates)
- 📋 **Tax-gain and tax-loss harvesting by lot** — book gains inside India's ₹1.25 lakh LTCG
  exemption before 31 March, harvest losses with the STCL/LTCL set-off rules, and flag the US
  wash-sale window (30 days either side). Smaller first step: `harvest --json`/`--csv`
  ([#21](https://github.com/iamsaquib8/stockwise/issues/21))
- 📋 **Advance tax** — quarterly estimates for the 15 Jun, 15 Sep, 15 Dec and 15 Mar instalments
- 📋 **Indian ITR toolkit** — 31 Jan 2018 grandfathering from fair market values (flagged only
  today), Schedule FA for foreign holdings (calendar-year peak and closing values in INR),
  Schedule FSI/TR and Form 67 for tax withheld on US dividends, debt funds under Sec. 50AA, SGB
  redemption, crypto under Sec. 115BBH; and the Income-tax Act 2025's "tax year" wording and new
  forms once they are notified
- 📋 **Dividend and interest income** with TDS across stocks, mutual funds (CAS IDCW rows), SGBs,
  REITs/InvITs (split into interest, dividend and capital repayment) and bonds

**Portfolio**
- 📋 **Target-allocation and inflow-only rebalancing** — saved target weights by stock, sector or
  asset class, drift bands, and "buy-only" rebalancing for new cash or SIP inflows (today
  `rebalance` is equal-weight only)
- 📋 **Full net worth, SIP tracker and household view** — NPS, PPF, EPF, FDs and cash next to the
  portfolio; saved goals with running SIPs tracked against them; several portfolios (family
  members) rolled into one view
- 📋 **Portfolio X-Ray** — look-through overlap across stocks, ETFs and funds, plus asset-class,
  market-cap and sector concentration (Indian fund look-through needs AMC monthly disclosures)

**Research**
- 📋 **Screener queries** — `stockwise screen "roce > 20 and pe < 25"` with custom ratios, the
  NIFTY Midcap 150 / Smallcap 250 / 500 universes ([#16](https://github.com/iamsaquib8/stockwise/issues/16)), and importing your own screener
  exports
- 📋 **Open-interest analytics** — OI change and build-up, PCR and IV history, IV percentile
- 📋 **Virtual F&O trading** in the simulator, with lot sizes and margin estimates
- 📋 **Market mood** — a transparent composite of India VIX, breadth and momentum, plus FII/DII
  cash flows
- 📋 **Open scorecards** — Piotroski F-score, Altman Z and quality/value/momentum, with the formulas
  shown
- 📋 **Mutual fund depth** — rolling returns, category rank and direct-vs-regular cost

**Markets**
- 📋 **More exchanges** in the market table — South Korea (`-m kr`, `.KS` and KOSDAQ `.KQ`), Taiwan
  (`-m tw`, `.TW` and `.TWO`), France (`.PA`), Netherlands (`.AS`), Switzerland (`.SW`), China
  (`.SS`/`.SZ`) and Brazil (`.SA`), each with its currency ([#17](https://github.com/iamsaquib8/stockwise/issues/17)), hours and holidays
- 📋 **Energy and industrial metals** in `-m commodity` — Brent and WTI crude (USD and ₹/bbl),
  natural gas, copper, platinum (metals: [#20](https://github.com/iamsaquib8/stockwise/issues/20))
- 📋 **Macro** — US Treasury yields (3M, 5Y, 10Y, 30Y) and the India–US 10Y spread in `bonds` and
  `markets` ([#19](https://github.com/iamsaquib8/stockwise/issues/19)), DXY and NRI cross-rates (AED, SGD, JPY, AUD, CAD, CHF vs INR) in
  `-m fx` ([#18](https://github.com/iamsaquib8/stockwise/issues/18)), and an
  Indian REIT/InvIT list (these already quote by symbol; this adds names, aliases and lists)

**AI**
- 📋 **`--explain` for beginners** — plain-English notes under technicals, risk metrics, option greeks
  and tax numbers (templates first, the local model optional)

**Everywhere**
- 📋 **Two-way Telegram bot and scheduled digests** — morning and post-market summaries, results
  and ex-dividend dates, GTT changes, and commands from your phone
- 📋 **More brokers through OpenAlgo** — one REST adapter to the brokers it supports
- 📋 **Installable web dashboard and richer TUI** — phone install (needs TLS for the service
  worker), an in-app portfolio switcher, a command palette and keyboard shortcuts, and sharper
  charts on terminals with image support

## 💡 Later — exploring (0.7+)

- 💡 **Options strategy backtests** on end-of-day chains
- 💡 **Trade journal, position sizer and two-leg (OCO) GTTs** — notes and tags on lots and sales,
  win-rate/expectancy, an R-multiple sizer (`stoploss` sizing: [#22](https://github.com/iamsaquib8/stockwise/issues/22))
- 💡 **Ownership signals** — well-known investors' portfolios, shareholding and pledge changes,
  alerts on phrases in filings, and local-LLM summaries of earnings calls
- 💡 **AI in the dashboards** — an assistant pane in the TUI and web dashboard on top of the same
  tool layer
- 💡 **Editor and launcher extensions** — VS Code status bar, Raycast; an npm wrapper
  (`npx stockwise mcp`)
- 💡 **Plugins and galleries** — WASM plugins for importers and data providers, and community
  simulator scenarios
- 💡 **Gulf exchanges** — Saudi Tadawul (`-m sa`, Sun–Thu week) and UAE DFM/ADX (`-m ae`)
- 💡 **MCX futures and live individual G-sec / T-bill prices** — via optional broker feeds (no
  free public feed exists)

## Non-goals

- Becoming a brokerage or holding custody of funds
- Promising profits or providing personalized financial advice (see
  [DISCLAIMER.md](DISCLAIMER.md)), stock tips or recommended baskets
- High-frequency / co-located trading
- Scraping or redistributing licensed data (consensus estimates, paid screener datasets); the
  NSE/BSE website provider stays opt-in because the exchanges' terms restrict automated
  collection
- IPO grey-market premiums and Account Aggregator integration (needs an RBI licence)
- AI that trades on its own — any order an AI tool proposes goes through the same preview and
  typed confirmation as a human's
