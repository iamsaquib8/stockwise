# Roadmap

Where StockWise is headed. Dates are intentionally omitted — this is a
direction, not a promise. Have an opinion? Open a
[Discussion](https://github.com/iamsaquib8/stockwise/discussions) or upvote an
[issue](https://github.com/iamsaquib8/stockwise/issues) — the roadmap follows
demand.

Legend: ✅ shipped · 🛠️ in progress · 📋 planned · 💡 exploring

## ✅ Shipped

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

## 🚀 Reach — get StockWise to more people (0.5.x)

StockWise has the features; most people who would use it haven't found it yet.
This comes first.

- ✅ **Documentation site** on GitHub Pages with a one-page [cheatsheet](docs/cheatsheet.md) and an
  honest [comparison](docs/comparison.md) with other tools
- 🛠️ **Homebrew tap, Scoop bucket, winget and AUR** — manifests are generated on every release;
  publishing them needs the tap repository and package submissions. Then homebrew-core once
  eligible (75★ when a user submits it)
- ✅ **`stockwise mcp` — an MCP server for AI assistants** — Claude, Cursor and other agents use
  quotes, history and technicals, risk, fund NAVs and SIP returns, option chains and payoffs,
  screens, backtests, your portfolio, lots, tax estimates, the calendar and goal calculators as 24
  read-only tools; portfolio, watchlist and alerts as resources; prompts for a portfolio review, a
  tax-harvest check before 31 March and an option-chain explainer. Stdio and Streamable HTTP. No
  live orders: `--allow-orders` only adds a dry-run preview that hands you the `stockwise trade`
  command. 🛠️ Next: list it in the official MCP registry (`server.json` is ready) and
  awesome-mcp-servers, and attach Claude Desktop extensions (`packaging/mcpb/`) to releases
  (Zerodha, Groww, Dhan, Upstox and Fyers ship MCP servers; Angel One doesn't)
- ✅ **Findable by AI assistants** — `llms.txt` and `llms-full.txt` on the docs site and an Agent
  Skill (`skills/stockwise/SKILL.md`) so Claude Code and other agents know how to use StockWise
- 📋 **`pipx install stockwise` / `uvx stockwise`** — PyPI wheels carrying the binary (maturin
  `bin`), where India's Python trading community already installs its tools
- 📋 **Animated demo** (VHS) at the top of the README and terminal recordings in the docs
- 📋 **More places to install** — nixpkgs, Termux on Android, `.deb`/`.rpm` release assets, and a
  Docker Hub mirror of the GHCR image
- 📋 **Try it without installing** — a Codespaces/devcontainer setup and a read-only hosted demo
  that runs on bundled historical data
- 📋 **Daily replay challenge** with a shareable image card, built on the simulator's scoring
- 📋 **Download stats without telemetry** — a nightly job that archives crates.io, GitHub release,
  Homebrew, Docker and PyPI counts and GitHub traffic referrers

## 🛠️ Now — finish what 0.5 started (0.5.x)

- 📋 **Mutual funds and SGBs in portfolio valuation** — price `MF:<AMFI code|ISIN>` holdings (from
  `import cas`) from AMFI's end-of-day NAVs and SGBs from NSE across `portfolio`, `dashboard`,
  `tui`, `serve`, `wealth`, `rebalance` and `tax` (`portfolio benchmark` and `tax export` already
  handle fund codes), and stop `portfolio add`, `watch add` and `alert add` turning `MF:123` into
  `MF:123.NS`
- 📋 **Direct `.xlsx` imports** — teach the built-in reader dates and sheet selection, and read
  Groww, Zerodha Console and Upstox `.xlsx` holdings and tradebook exports without saving as CSV;
  add an Angel One import
- 🛠️ **Broker integrations verified against live accounts** — the read-only `trade selftest` and the
  [verification criteria](docs/brokers.md#help-verify-a-broker) are ready; we need 3 reports per
  broker plus one small live order a tester places themselves
- 📋 **Educational AI wording** — reword `report` and `--ai` output from buy/hold/sell "verdicts" to
  neutral analysis ("signals point to…, risks are…") with a disclaimer, in line with SEBI's
  research-analyst and finfluencer rules
- 📋 **SEBI retail-algo rules** (in force since 1 Apr 2026) in `trade` — order-rate limits,
  price-protected market orders, daily login and static-IP guidance
- 📋 **Check the NSE/BSE parsers against live responses** and add SENSEX from BSE (the endpoints are
  unofficial; licensed exchange feeds stay out of scope)

## 📋 Next — your money, end to end, and more markets (0.6)

**Tax season** (ship before the 15 Dec and 15 Mar advance-tax dates)
- 📋 **Tax-gain and tax-loss harvesting by lot** — book gains inside India's ₹1.25 lakh LTCG
  exemption before 31 March, harvest losses with the STCL/LTCL set-off rules, and flag the US
  wash-sale window (30 days either side)
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
  NIFTY Midcap 150 / Smallcap 250 / 500 universes, and importing your own screener exports
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
  (`.SS`/`.SZ`) and Brazil (`.SA`), each with its currency, hours and holidays
- 📋 **Energy and industrial metals** in `-m commodity` — Brent and WTI crude (USD and ₹/bbl),
  natural gas, copper, platinum
- 📋 **Macro** — US Treasury yields (3M, 5Y, 10Y, 30Y) and the India–US 10Y spread in `bonds` and
  `markets`, DXY and NRI cross-rates (AED, SGD, JPY, AUD, CAD, CHF vs INR) in `-m fx`, and an
  Indian REIT/InvIT list (these already quote by symbol; this adds names, aliases and lists)

**AI**
- ✅ **One tool layer for agents** — the same typed tool definitions power `stockwise mcp` and
  `stockwise ask "which of my holdings fell most this month?"`, where a local Ollama model (with
  tool calling) or an opt-in cloud model picks the tools, StockWise runs them and the model answers
  in plain English
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
  win-rate/expectancy, an R-multiple sizer
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
