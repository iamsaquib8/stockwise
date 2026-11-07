# Roadmap

The plan from today to a public 1.0, then what comes after. Each release has a goal, the issues
in it, and a test for "done". Target dates are aims, not promises: a release ships when its
"done when" list is true. Have an opinion? Open a
[Discussion](https://github.com/iamsaquib8/stockwise/discussions) or upvote an
[issue](https://github.com/iamsaquib8/stockwise/issues) — the roadmap follows demand.

The live view is on GitHub: [milestones](https://github.com/iamsaquib8/stockwise/milestones) for
releases and six epics for the themes:

| Epic | Goal |
|---|---|
| [#257](https://github.com/iamsaquib8/stockwise/issues/257) Install anywhere | One command on every platform, updates included |
| [#258](https://github.com/iamsaquib8/stockwise/issues/258) Trustworthy portfolio data | Imports never lose or corrupt lots |
| [#259](https://github.com/iamsaquib8/stockwise/issues/259) Tax season 2026-27 | Capital gains, advance tax and income ready for ITR |
| [#260](https://github.com/iamsaquib8/stockwise/issues/260) Neutral, accurate research | Data and transparent scores, no buy/sell calls |
| [#261](https://github.com/iamsaquib8/stockwise/issues/261) StockWise for AI assistants | Ask about your portfolio, data kept local |
| [#262](https://github.com/iamsaquib8/stockwise/issues/262) A simple, stable 1.0 | A small CLI and a written compatibility promise |

Legend: ✅ shipped · 🛠️ in progress · 📋 planned · 💡 exploring

## ✅ Shipped

**0.8** — portfolio depth, tax season and an easier start
- Portfolio X-Ray (look-through overlap across stocks, funds and ETFs), target allocations with
  drift bands and inflow-only rebalancing, and lot-level `harvest` with `--json`/`--csv`
- Advance tax instalments (Section 211/234C); open-interest analytics and build-up; transparent
  Piotroski F-score and Altman Z scorecards; an Indian IPO workbench
- Direct `.xlsx` broker imports; `stockwise SYMBOL`, short command names and name aliases;
  `--explain` on `verdict`, `technical`, `score` and `options oi`
- Eight more MCP/`ask` tools; a research workbench in the web dashboard; TUI stock inspector

**0.7** — key events timeline, quantified insights and decision engine
- `stockwise events`: NSE corporate filings + RSS news timeline with regex financial extraction,
  benchmark-adjusted market reactions and upcoming corporate radar (`-m in` and `-m us`)
- `stockwise insights`: INDmoney-style positive/negative/neutral impact cards across 9 dimensions,
  with live NSE XBRL shareholding (FII/DII/promoter/pledges) and US SEC insider & institutional data
- `stockwise verdict`: 6-pillar 0–100 decision engine with entry zone, stop-loss, target price,
  risk-reward ratio, multi-horizon guidance and bull vs. bear thesis

**0.6** — AI assistants and more ways to install (first released in 0.7.0)
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

## The road to 1.0

Who 1.0 is for: an investor in India who is comfortable in a terminal or an AI assistant, holds
stocks and mutual funds across brokers, files their own ITR, and wants their data on their own
machine. Four jobs must be excellent: **one true portfolio**, **capital-gains tax done right**,
**fast lookups**, and **asking an AI about your money locally**. Everything else is secondary
until 1.0.

### 0.8.1 — fix what's wrong ([milestone](https://github.com/iamsaquib8/stockwise/milestone/1), target: late November 2026)

Correct numbers before new features. These are the "Known issues" in the 0.8.0 release.

- 📋 **No data loss on import** — a second tradebook or holdings file replacing lots
  ([#83](https://github.com/iamsaquib8/stockwise/issues/83), [#85](https://github.com/iamsaquib8/stockwise/issues/85)), same-day trades booked as capital gains ([#84](https://github.com/iamsaquib8/stockwise/issues/84)), merger swap ratios ([#86](https://github.com/iamsaquib8/stockwise/issues/86))
- 📋 **Right tax and numbers** — non-equity ETFs taxed as shares ([#87](https://github.com/iamsaquib8/stockwise/issues/87)), the SIP projection
  ([#67](https://github.com/iamsaquib8/stockwise/issues/67)), USD statements shown in ₹ ([#207](https://github.com/iamsaquib8/stockwise/issues/207)), NSE dividend yield ([#208](https://github.com/iamsaquib8/stockwise/issues/208)), BSE codes not
  found ([#206](https://github.com/iamsaquib8/stockwise/issues/206))
- 📋 **No buy/sell calls** — neutral pillar scores instead of verdicts, entries and targets
  ([#188](https://github.com/iamsaquib8/stockwise/issues/188), [#112](https://github.com/iamsaquib8/stockwise/issues/112))
- 📋 **Works when Yahoo hiccups** — open the session lazily ([#154](https://github.com/iamsaquib8/stockwise/issues/154))
- 📋 **Releases publish themselves** — crates.io and PyPI from the release workflow ([#169](https://github.com/iamsaquib8/stockwise/issues/169))

Done when: no open `priority: high` issues.

### 0.9 — install anywhere, ready for tax season ([milestone](https://github.com/iamsaquib8/stockwise/milestone/2), target: before 15 December 2026)

The 15 December advance-tax instalment is the first moment Indian investors look for tax tools.

- 📋 **One-command installs** ([#257](https://github.com/iamsaquib8/stockwise/issues/257)) — npm and `npx -y stockwise mcp` ([#228](https://github.com/iamsaquib8/stockwise/issues/228)), PyPI
  ([#169](https://github.com/iamsaquib8/stockwise/issues/169)), Homebrew ([#233](https://github.com/iamsaquib8/stockwise/issues/233)), winget ([#234](https://github.com/iamsaquib8/stockwise/issues/234)), AUR ([#235](https://github.com/iamsaquib8/stockwise/issues/235)), a Scoop bucket ([#236](https://github.com/iamsaquib8/stockwise/issues/236)),
  nixpkgs and Termux ([#237](https://github.com/iamsaquib8/stockwise/issues/237)), and the Docker Hub mirror ([#271](https://github.com/iamsaquib8/stockwise/issues/271))
- 📋 **AI setup in a minute** — `stockwise mcp setup <client>` ([#193](https://github.com/iamsaquib8/stockwise/issues/193)), the official MCP
  registry ([#194](https://github.com/iamsaquib8/stockwise/issues/194)), MCP directories and awesome lists ([#242](https://github.com/iamsaquib8/stockwise/issues/242))
- 📋 **Tax season** ([#259](https://github.com/iamsaquib8/stockwise/issues/259)) — consistent asset classes ([#88](https://github.com/iamsaquib8/stockwise/issues/88)), advance-tax payments with dates
  ([#89](https://github.com/iamsaquib8/stockwise/issues/89)) and cess ([#98](https://github.com/iamsaquib8/stockwise/issues/98)), pre-July-2024 rates by sale date ([#99](https://github.com/iamsaquib8/stockwise/issues/99)), harvesting with FIFO and set-off ([#91](https://github.com/iamsaquib8/stockwise/issues/91), [#92](https://github.com/iamsaquib8/stockwise/issues/92)),
  pre-2018 grandfathering ([#247](https://github.com/iamsaquib8/stockwise/issues/247)), dividend and interest income with TDS ([#246](https://github.com/iamsaquib8/stockwise/issues/246)), and
  `tax reconcile` against the broker's tax P&L ([#203](https://github.com/iamsaquib8/stockwise/issues/203))
- 📋 **Soft launch** — a how-to post on r/IndiaInvestments before 15 December ([#200](https://github.com/iamsaquib8/stockwise/issues/200))

Done when: every channel in "Coming soon" on the install page is live, and a sample portfolio's
Schedule 112A export matches a hand-checked calculation.

### 0.10 — simplify and stabilise: the 1.0 release candidate ([milestone](https://github.com/iamsaquib8/stockwise/milestone/4), target: January 2027)

Feature freeze. Make the CLI small, the data safe and the promise written.

- 📋 **A smaller CLI** — the simplification plan ([#205](https://github.com/iamsaquib8/stockwise/issues/205)): about 25 commands in the short help,
  old names kept as hidden aliases for one release; help that fits a terminal ([#150](https://github.com/iamsaquib8/stockwise/issues/150))
- 📋 **A first run that works** — `stockwise init` ([#192](https://github.com/iamsaquib8/stockwise/issues/192)) and a README written for one user and
  four jobs ([#191](https://github.com/iamsaquib8/stockwise/issues/191))
- 📋 **Safe data** ([#258](https://github.com/iamsaquib8/stockwise/issues/258)) — locked, atomic writes ([#93](https://github.com/iamsaquib8/stockwise/issues/93)), versioned files with migrations and
  backups ([#256](https://github.com/iamsaquib8/stockwise/issues/256)), editing lots and corporate actions ([#94](https://github.com/iamsaquib8/stockwise/issues/94), [#95](https://github.com/iamsaquib8/stockwise/issues/95), [#96](https://github.com/iamsaquib8/stockwise/issues/96))
- 📋 **Neutral, accurate research** ([#260](https://github.com/iamsaquib8/stockwise/issues/260)) and **AI answers checked by an eval set** ([#255](https://github.com/iamsaquib8/stockwise/issues/255))
- 📋 **A written promise** — the 1.0 stability policy ([#195](https://github.com/iamsaquib8/stockwise/issues/195)) and stable `--json` shapes
  ([#223](https://github.com/iamsaquib8/stockwise/issues/223), [#180](https://github.com/iamsaquib8/stockwise/issues/180))
- 📋 **Quality gates** — required CI ([#171](https://github.com/iamsaquib8/stockwise/issues/171)), cargo-deny ([#172](https://github.com/iamsaquib8/stockwise/issues/172)), hardened workflows
  ([#173](https://github.com/iamsaquib8/stockwise/issues/173)), CLI snapshot tests ([#181](https://github.com/iamsaquib8/stockwise/issues/181)), parser fuzzing ([#182](https://github.com/iamsaquib8/stockwise/issues/182)), a weekly live-data canary
  ([#245](https://github.com/iamsaquib8/stockwise/issues/245))

Done when: the release candidate runs a full user journey (install, init, import, portfolio, tax,
MCP) on macOS, Linux and Windows without a known bug in the four jobs.

### 1.0 — public launch ([milestone](https://github.com/iamsaquib8/stockwise/milestone/3), target: February 2027)

Launch before the 31 March harvesting deadline and the ITR season, when the four jobs matter most.

- 📋 **Show it** — an animated demo in the README ([#23](https://github.com/iamsaquib8/stockwise/issues/23)), "Open in Codespaces" ([#240](https://github.com/iamsaquib8/stockwise/issues/240)), three
  task guides for the core jobs ([#201](https://github.com/iamsaquib8/stockwise/issues/201))
- 📋 **Launch** — Show HN ([#199](https://github.com/iamsaquib8/stockwise/issues/199)), Reddit and TradingQnA posts timed to the harvest window
  ([#200](https://github.com/iamsaquib8/stockwise/issues/200)), Product Hunt last ([#272](https://github.com/iamsaquib8/stockwise/issues/272))
- 📋 **Measure** — a launch log and before/after numbers ([#204](https://github.com/iamsaquib8/stockwise/issues/204)); targets for the 90 days after
  launch: 150 stars, 150 installs a week, 500 unique visitors per 14 days

Done when: 1.0 is tagged, the stability page is published, and the launch posts are out.

## 💡 After 1.0

Planned, but only once the four jobs are solid. Ordered roughly by demand.

**Your money**
- 📋 **Net worth** — PPF, EPF, NPS, FDs and cash next to the portfolio ([#248](https://github.com/iamsaquib8/stockwise/issues/248)); SIP tracking and a
  household view
- 📋 **Indian ITR toolkit, rest** ([#264](https://github.com/iamsaquib8/stockwise/issues/264)) — Schedule FA for foreign holdings, Form 67 for US dividend tax,
  debt funds under Sec. 50AA, crypto under Sec. 115BBH, and the Income-tax Act 2025's new forms
- 📋 **Read-only broker sync** of holdings and trades instead of CSV imports ([#254](https://github.com/iamsaquib8/stockwise/issues/254)), and quotes
  from the user's own broker API ([#253](https://github.com/iamsaquib8/stockwise/issues/253))

**Everyday use**
- 📋 **Alerts that run in the background** as an OS service ([#252](https://github.com/iamsaquib8/stockwise/issues/252))
- 📋 **A local price-history store** so charts and backtests work offline and fast ([#251](https://github.com/iamsaquib8/stockwise/issues/251))
- 📋 **Two-way Telegram bot and scheduled digests** ([#265](https://github.com/iamsaquib8/stockwise/issues/265))
- 📋 **More install and launcher options** — mise/aqua ([#238](https://github.com/iamsaquib8/stockwise/issues/238)), a signed APT/RPM repository
  ([#239](https://github.com/iamsaquib8/stockwise/issues/239)), a GitHub Action ([#241](https://github.com/iamsaquib8/stockwise/issues/241)), a Raycast extension ([#243](https://github.com/iamsaquib8/stockwise/issues/243))

**Research**
- 📋 **Screener queries** ([#266](https://github.com/iamsaquib8/stockwise/issues/266)) — `stockwise screen "roce > 20 and pe < 25"`, the NIFTY Midcap 150 /
  Smallcap 250 / 500 universes ([#16](https://github.com/iamsaquib8/stockwise/issues/16))
- 📋 **Market mood** ([#249](https://github.com/iamsaquib8/stockwise/issues/249)) and **mutual fund depth** — rolling returns, category rank,
  direct-vs-regular cost ([#250](https://github.com/iamsaquib8/stockwise/issues/250))
- 📋 **`--explain` everywhere** — `risk`, option greeks and `tax` ([#52](https://github.com/iamsaquib8/stockwise/issues/52))
- 📋 **Symbol discovery** — suggestions in every command, a picker and symbol completion ([#54](https://github.com/iamsaquib8/stockwise/issues/54))

**Markets**
- 📋 **More exchanges** ([#267](https://github.com/iamsaquib8/stockwise/issues/267)) — South Korea, Taiwan, France, Netherlands, Switzerland, China and Brazil,
  each with its currency ([#17](https://github.com/iamsaquib8/stockwise/issues/17)), hours and holidays
- 📋 **Energy and industrial metals** ([#20](https://github.com/iamsaquib8/stockwise/issues/20)) and **macro** — US yields ([#19](https://github.com/iamsaquib8/stockwise/issues/19)), DXY and NRI
  cross-rates ([#18](https://github.com/iamsaquib8/stockwise/issues/18)), an Indian REIT/InvIT list

**Practice and trading**
- 📋 **Daily replay challenge** with a shareable card ([#244](https://github.com/iamsaquib8/stockwise/issues/244))
- 📋 **Virtual F&O trading** in the simulator, with lot sizes and margin estimates ([#268](https://github.com/iamsaquib8/stockwise/issues/268))
- 🛠️ **Broker integrations verified against live accounts** ([#269](https://github.com/iamsaquib8/stockwise/issues/269), help wanted) — `trade selftest` and the
  [verification criteria](docs/brokers.md#help-verify-a-broker) are ready; we need 3 reports per
  broker plus one small live order a tester places themselves
- 📋 **SEBI retail-algo rules** in `trade` — order-rate limits, algo tagging and static-IP setup
  ([#132](https://github.com/iamsaquib8/stockwise/issues/132))
- 📋 **More brokers through OpenAlgo** — one REST adapter to the brokers it supports ([#270](https://github.com/iamsaquib8/stockwise/issues/270))

**Exploring**
- 💡 Options strategy backtests on end-of-day chains ([#273](https://github.com/iamsaquib8/stockwise/issues/273))
- 💡 Trade journal and two-leg (OCO) GTTs; position sizing in `stoploss` ([#22](https://github.com/iamsaquib8/stockwise/issues/22))
- 💡 Ownership signals — shareholding and pledge changes, alerts on phrases in filings ([#274](https://github.com/iamsaquib8/stockwise/issues/274))
- 💡 An assistant pane in the TUI and web dashboard ([#275](https://github.com/iamsaquib8/stockwise/issues/275))
- 💡 Gulf exchanges; MCX futures and live G-sec prices via optional broker feeds ([#276](https://github.com/iamsaquib8/stockwise/issues/276))
- 💡 WASM plugins for importers and data providers — kept experimental until someone uses them ([#277](https://github.com/iamsaquib8/stockwise/issues/277))

## Non-goals

- Becoming a brokerage or holding custody of funds
- Promising profits or providing personalized financial advice (see
  [DISCLAIMER.md](DISCLAIMER.md)), stock tips or recommended baskets
- High-frequency / co-located trading
- Scraping or redistributing licensed data (consensus estimates, paid screener datasets); the
  NSE/BSE website provider stays opt-in because the exchanges' terms restrict automated
  collection
- A licensed or scraped IPO grey-market premium feed, and Account Aggregator integration (needs an
  RBI licence); `stockwise ipo` only shows GMP figures that news headlines report, as unofficial
- AI that trades on its own — any order an AI tool proposes goes through the same preview and
  typed confirmation as a human's
