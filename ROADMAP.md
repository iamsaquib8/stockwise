# Roadmap

The plan from today to a public 1.0, then what comes after. Each release has a goal, the issues
in it, and a test for "done". Target dates are aims, not promises: a release ships when its
"done when" list is true. Have an opinion? Open a
[Discussion](https://github.com/iamsaquib8/stockwise/discussions) or upvote an
[issue](https://github.com/iamsaquib8/stockwise/issues) — the roadmap follows demand.

The live view is on GitHub: [milestones](https://github.com/iamsaquib8/stockwise/milestones) for
releases, and eleven epics for the themes. Every open issue is a sub-issue of exactly one epic, so
each epic shows its full list and progress.

| Epic | Goal | Until |
|---|---|---|
| [#257](https://github.com/iamsaquib8/stockwise/issues/257) Install anywhere | One command on every platform, updates included | 0.9 |
| [#258](https://github.com/iamsaquib8/stockwise/issues/258) Trustworthy portfolio data | Multi-currency & multi-broker imports never lose or corrupt lots | 0.10 |
| [#259](https://github.com/iamsaquib8/stockwise/issues/259) Global & Indian tax season | Capital gains, lot engines (`FIFO`/`HIFO`/`Sec 104`/`ACB`), wrappers & tax exports | 0.9 |
| [#260](https://github.com/iamsaquib8/stockwise/issues/260) Neutral, accurate research | Data and transparent scores, no buy/sell calls, correct numbers | 0.10 |
| [#261](https://github.com/iamsaquib8/stockwise/issues/261) StockWise for AI assistants | Ask about your portfolio, data kept local | 0.10 |
| [#262](https://github.com/iamsaquib8/stockwise/issues/262) A simple, stable 1.0 | A small CLI ([#205](https://github.com/iamsaquib8/stockwise/issues/205)), a compatibility promise, quality gates | 0.10 |
| [#278](https://github.com/iamsaquib8/stockwise/issues/278) Reliable market data | Sources that recover, fast commands, a live-data canary | 0.10 |
| [#279](https://github.com/iamsaquib8/stockwise/issues/279) Security and trading safety | Private secrets, safe local servers, guarded live orders | ongoing |
| [#280](https://github.com/iamsaquib8/stockwise/issues/280) Dashboards, alerts and practice | A dependable TUI, web dashboard, alerts and simulator | ongoing |
| [#281](https://github.com/iamsaquib8/stockwise/issues/281) Launch and community | Be found, be tried in a minute, hear from users | 1.0 |
| [#282](https://github.com/iamsaquib8/stockwise/issues/282) More markets and deeper research | Coverage and research depth users ask for | after 1.0 |

Legend: ✅ shipped · 🛠️ in progress · 📋 planned · 💡 exploring

## ✅ Shipped

**0.8.1** — fix what was wrong
- Imports no longer lose lots: a second tradebook or broker file merges, same-day trades are
  intraday, HDFC and Mindtree trades use the merger ratios, and writes are locked
- Gold, silver, liquid and international ETFs are no longer taxed as listed shares
- Correct SIP projection, BSE codes reported as not found, USD statements converted, dividend yield
- `verdict`, `ipo`, `picks` and `intraday` show scores and data, never buy/sell calls
- Yahoo's session opens lazily, so one failed handshake no longer stops every command
- Tax: 4% cess, dated advance-tax payments, one classification across `tax in`, `tax export in`,
  `tax advance` and `harvest`, sale-date rates in the export, and FIFO-aware `harvest`
- Calculations: put build-up for the underlying, date-aligned correlation, 365-bar crypto
  annualisation, prior-session pivots, dollar size cut-offs
- Alerts, simulator and trade safety fixes; `--yes` orders need `max_order_value`
- Scorecards count only the signals that have data; statements in another currency convert
  correctly; neutral wording in `technical`, `ipo`, `longterm`, the report and the daemon
- Docs site: a Troubleshooting page, README and docs checked against the binary; changelog
  fragments replace edits to `CHANGELOG.md`

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

Who 1.0 is for: a self-directed global investor or developer — across the US, UK, Europe, Canada,
Australia, APAC, and India (including cross-border expats and NRIs) — who is comfortable in a
terminal or an AI assistant, holds stocks, ETFs and funds across brokers and currencies, files
their own taxes, and wants their financial data kept strictly private on their own machine. Four
jobs must be excellent everywhere: **one true multi-currency portfolio**, **local capital-gains tax
done right**, **instant keyless market lookups**, and **asking an AI about your money locally**.
Everything else is secondary until 1.0.

### 0.9 — global-first markets, `stockwise use` switcher, investor profiles, multi-jurisdiction tax & year-end checkup ([milestone](https://github.com/iamsaquib8/stockwise/milestone/2), target: 31 December 2026)

Epics: [#257](https://github.com/iamsaquib8/stockwise/issues/257), [#258](https://github.com/iamsaquib8/stockwise/issues/258), [#259](https://github.com/iamsaquib8/stockwise/issues/259), [#260](https://github.com/iamsaquib8/stockwise/issues/260), [#261](https://github.com/iamsaquib8/stockwise/issues/261), [#262](https://github.com/iamsaquib8/stockwise/issues/262), [#278](https://github.com/iamsaquib8/stockwise/issues/278), [#279](https://github.com/iamsaquib8/stockwise/issues/279), [#280](https://github.com/iamsaquib8/stockwise/issues/280), [#281](https://github.com/iamsaquib8/stockwise/issues/281), [#282](https://github.com/iamsaquib8/stockwise/issues/282).

Year-end (31 Dec US/EU/CA tax-loss harvesting cutoff, December US/Indian estimated/advance tax, and Q1 UK ISA / Indian Jan–Mar tax windows) is when global and Indian investors look for local-first portfolio, tax, and goal tools.

- 📋 **One-command installs** ([#257](https://github.com/iamsaquib8/stockwise/issues/257)) — npm and `npx -y stockwise mcp` ([#228](https://github.com/iamsaquib8/stockwise/issues/228)), PyPI
  ([#169](https://github.com/iamsaquib8/stockwise/issues/169)), Homebrew ([#233](https://github.com/iamsaquib8/stockwise/issues/233)), winget ([#234](https://github.com/iamsaquib8/stockwise/issues/234)), AUR ([#235](https://github.com/iamsaquib8/stockwise/issues/235)), a Scoop bucket ([#236](https://github.com/iamsaquib8/stockwise/issues/236)),
  nixpkgs and Termux ([#237](https://github.com/iamsaquib8/stockwise/issues/237)), the Docker Hub mirror ([#271](https://github.com/iamsaquib8/stockwise/issues/271)), and hardened `install.sh` checksum verification ([#130](https://github.com/iamsaquib8/stockwise/issues/130))
- 📋 **Instant market switcher (`stockwise use`), investor profiles, zero-`-m` resolution & visual demo** — **`stockwise use <market>` / `use -` / interactive session picker / `--shell` / `--status` prompt badge / `markets world` / `config set` and per-portfolio market binding** ([#388](https://github.com/iamsaquib8/stockwise/issues/388)), `stockwise init` ([#192](https://github.com/iamsaquib8/stockwise/issues/192)) from install to first portfolio, **Investor Profile modes** (`passive`, `active` with `low|med|high` risk, and `ultra` full-telemetry mode) with personal tax-slab & concentration guardrails ([#374](https://github.com/iamsaquib8/stockwise/issues/374)), zero-`-m` cross-market ticker/`EXCHANGE:SYM`/ISIN resolution, aliases and completion ([#54](https://github.com/iamsaquib8/stockwise/issues/54), [#388](https://github.com/iamsaquib8/stockwise/issues/388)), and the animated VHS terminal demo in the README ([#23](https://github.com/iamsaquib8/stockwise/issues/23))
- 📋 **Multi-currency `base_currency` portfolio & global broker imports** ([#389](https://github.com/iamsaquib8/stockwise/issues/389)) — trade-date historical FX cost basis (auto-backfilled via ECB/Frankfurter [#341](https://github.com/iamsaquib8/stockwise/issues/341)), `Local Return + FX Impact + Net Dividends` attribution, **`TTWROR` (time-weighted) alongside `XIRR`**, `GBp` minor-unit & `WHT`/`DRIP` dividend ledger, and 1-command importers for **Interactive Brokers (IBKR Flex/CSV), Schwab, Fidelity, Robinhood, Vanguard, Trading 212, DEGIRO, Scalable Capital, Wealthsimple, Portfolio Performance (`.xml`/CSV) and Sharesight** alongside Zerodha, Groww, Upstox and PDF CAS ([#375](https://github.com/iamsaquib8/stockwise/issues/375))
- 📋 **Universal `--json` & `--csv` serialization parity, agentic `ask --auto` & MCP** — 100% `--json` and `--csv` coverage across all data/tabular commands with unified schemas and JSON errors ([#381](https://github.com/iamsaquib8/stockwise/issues/381), [#223](https://github.com/iamsaquib8/stockwise/issues/223), [#180](https://github.com/iamsaquib8/stockwise/issues/180)), `stockwise mcp setup <client>` ([#193](https://github.com/iamsaquib8/stockwise/issues/193)), the official MCP
  registry ([#194](https://github.com/iamsaquib8/stockwise/issues/194)), MCP directories ([#242](https://github.com/iamsaquib8/stockwise/issues/242)), expanded MCP tools ([#53](https://github.com/iamsaquib8/stockwise/issues/53)), and **profile-aware `stockwise ask --auto` / `-i` REPL with safe workspace actions (`stockwise undo`) and built-in playbooks (`checkup`, `tax-season`, `earnings`)** ([#378](https://github.com/iamsaquib8/stockwise/issues/378))
- 📋 **Global & Indian tax engines, tax-free wrappers & corporate actions** ([#259](https://github.com/iamsaquib8/stockwise/issues/259), [#390](https://github.com/iamsaquib8/stockwise/issues/390)) — pluggable lot-accounting engines (**US & AU `HIFO`/`LIFO`/`MinTax` SpecID**, **UK HMRC Section 104 Pool + 30-day Bed & Breakfast** with SA108, **Canada `ACB` + Superficial Loss + T1135**, **Germany *Abgeltungsteuer* + 30% ETF *Teilfreistellung***, and **SG/HK 0% CGT + WHT**), US 30-day wash sales, NIIT, IRS quarterly estimated tax & Form 8949 ([#97](https://github.com/iamsaquib8/stockwise/issues/97)), **tax-advantaged wrappers (`Roth IRA`, `401k`, `ISA`, `SIPP`, `TFSA`, `RRSP`, `NISA`)**, plus full Indian ITR readiness (non-equity ETF categories [#318](https://github.com/iamsaquib8/stockwise/issues/318), Jan 2018 grandfathering [#247](https://github.com/iamsaquib8/stockwise/issues/247), dividend/interest TDS [#246](https://github.com/iamsaquib8/stockwise/issues/246), lot edit/undo [#94](https://github.com/iamsaquib8/stockwise/issues/94), bonus lots [#95](https://github.com/iamsaquib8/stockwise/issues/95), demergers/renames [#96](https://github.com/iamsaquib8/stockwise/issues/96), `tax reconcile` [#203](https://github.com/iamsaquib8/stockwise/issues/203), and **`portfolio sell --preview` with LTCG countdown** [#375](https://github.com/iamsaquib8/stockwise/issues/375))
- 📋 **Global & local post-tax goal & FIRE planning** ([#356](https://github.com/iamsaquib8/stockwise/issues/356), [#392](https://github.com/iamsaquib8/stockwise/issues/392)) — horizon-matched return & local sovereign yield baselines (`US Treasury`, `ECB Bund`, `UK Gilt`, `JGB`, `RBI G-Sec`) with pre-goal glide paths, per-instalment local tax & wrapper math (`401k`/`Roth`/`ISA`/`TFSA`/`NISA`), `P10/P50/P90` shortfall odds, sub-year horizons (`--months`, `--by`), saved goals (`goals.json`) linked to `portfolio`, **Global FIRE modes (`4%` SWR, Coast-FIRE, Guyton-Klinger guardrails)**, retirement Year-1 crash checks, and `goal swp|emergency|prepay`
- 📋 **19+ global stock markets, SQLite WAL cache, background worker & universal cap tiers** ([#267](https://github.com/iamsaquib8/stockwise/issues/267), [#357](https://github.com/iamsaquib8/stockwise/issues/357), [#377](https://github.com/iamsaquib8/stockwise/issues/377)) — expand to **South Korea, Taiwan, France, Netherlands, Switzerland, China and Brazil** ([#267](https://github.com/iamsaquib8/stockwise/issues/267)), `DXY` & global FX pairs ([#18](https://github.com/iamsaquib8/stockwise/issues/18)), zero-setup `cache.db` (L2 HTTP cache replacing Redis, indexed `market_snapshots`, incremental daily `ohlcv_bars`, quarterly results, and `tasks` queue), market-aware **background live-update worker & OS service** (`stockwise worker` + live push for `tui`/`serve`), capability routing, shared NSE session ([#159](https://github.com/iamsaquib8/stockwise/issues/159)), curated global index constituents ([#337](https://github.com/iamsaquib8/stockwise/issues/337)), provenance footer ([#338](https://github.com/iamsaquib8/stockwise/issues/338)), SEC EDGAR ([#339](https://github.com/iamsaquib8/stockwise/issues/339)), Stooq ([#340](https://github.com/iamsaquib8/stockwise/issues/340)), Frankfurter FX ([#341](https://github.com/iamsaquib8/stockwise/issues/341)), `midcap150`/`smallcap250`/`nifty500` ([#16](https://github.com/iamsaquib8/stockwise/issues/16)), and FX-adjusted / SEBI-rank `Mega / Large / Mid / Small / Micro` badges (`--cap`)
- 📋 **Global sovereign yield curves, SEC Form 4/13F, S-1 IPOs, American options, UCITS ETFs & year-end checkup** ([#391](https://github.com/iamsaquib8/stockwise/issues/391), [#19](https://github.com/iamsaquib8/stockwise/issues/19), [#361](https://github.com/iamsaquib8/stockwise/issues/361), [#376](https://github.com/iamsaquib8/stockwise/issues/376), [#392](https://github.com/iamsaquib8/stockwise/issues/392)) — US Treasury (`1M`–`30Y` + TIPS + `2s10s`), Bund, Gilt & JGB curves with `Act/Act` & `Act/360` bond math, SEC EDGAR **Form 4 insider** & **13F institutional** flows, **US/Global S-1 IPO & lock-up board**, **American equity option pricing & `GEX`**, **UCITS (`Acc`/`Dist`) & global ETF intelligence**, 8–12Q `stockwise results <SYMBOL>` collation, plus **`portfolio checkup` (holistic red-flag, global TER bleed, non-US investor US-domicile 30% WHT / \$60k estate-tax vs UCITS check, Regular-vs-Direct MF bleed & asset-location scan) and `portfolio wrap [--redact]`**
- 📋 **Global (`Alpaca`, `IBKR`) & Indian multi-broker execution, statutory order-cost preview & streaming TUI** ([#382](https://github.com/iamsaquib8/stockwise/issues/382), [#383](https://github.com/iamsaquib8/stockwise/issues/383), [#392](https://github.com/iamsaquib8/stockwise/issues/392)) — **Alpaca Markets** (US equities/crypto paper + live + WebSocket stream) and **Interactive Brokers (`IBKR`)** alongside Zerodha, Angel One, Upstox, Dhan and Fyers, keyring TOTP / OAuth renewal and cached instrument lists ([#31](https://github.com/iamsaquib8/stockwise/issues/31)), full market-specific fee & rule breakdown (**US SEC/FINRA + `PDT` counter, UK `0.5%` SDRT, HK board lots, and Indian STT/GST** + slippage & breakeven) in `trade` previews, position sizing in `stoploss` ([#22](https://github.com/iamsaquib8/stockwise/issues/22)), and event-driven streaming TUI (`m` market hotkey, custom `[tui]` layouts, live option Greeks Δ, Γ, Θ, ν)
- 📋 **Institutional Backtest Engine v2, On-Demand ML Model Hub (`stockwise model`) & Strategy Base-Rate Atlas** ([#383](https://github.com/iamsaquib8/stockwise/issues/383), [#394](https://github.com/iamsaquib8/stockwise/issues/394), [#395](https://github.com/iamsaquib8/stockwise/issues/395)) — multi-timeframe (`5m`–`1wk`), shorting & fractional shares, ATR/vol-target/Kelly sizing, Chandelier/trailing stops, sovereign `Rf` cash sweep & `--after-tax` tearsheets, cross-asset benchmark/VIX regime filters, `filed_at` point-in-time fundamental rules, Purged/Embargoed walk-forward (`CPCV`) + **Deflated Sharpe (`DSR`)**, Monte Carlo drawdown distributions, `backtest compare` & `--universe` runs over `stockwise data sync`, empirical survivorship-free **Strategy Base-Rate Atlas**, and **on-demand local models** (`stockwise model pull`, built-in pure-Rust `GARCH(1,1)` + `HMM` regime, `chronos-bolt` quantile cones, `finbert` filing scorer, and `onnx:`/`lgbm:` BYO models with mandatory **`training_cutoff` leakage guards**)
- 📋 **Global & regional soft launch** — how-to, local-first privacy and year-end portfolio-checkup posts across `r/commandline`, `r/Bogleheads`, `r/UKPersonalFinance`, `r/Finanzen`, `r/PersonalFinanceCanada`, `r/algotrading`, `r/IndiaInvestments`, `r/developersIndia` and TradingQnA ([#200](https://github.com/iamsaquib8/stockwise/issues/200))

Done when: every channel in "Coming soon" on the install page is live, every data command emits `--json`/`--csv`, `stockwise init` + `stockwise use` + `profile` + `portfolio checkup` work end-to-end across global and Indian portfolios, and hand-checked reference portfolios pass tax exports across US (Form 8949 / `HIFO` / Wash Sale), UK (Section 104 / SA108), Canada (`ACB`), and India (Schedule 112A).

### 0.10 — simplify and stabilise: the 1.0 release candidate ([milestone](https://github.com/iamsaquib8/stockwise/milestone/4), target: January 2027)

Epics: [#262](https://github.com/iamsaquib8/stockwise/issues/262), [#258](https://github.com/iamsaquib8/stockwise/issues/258), [#260](https://github.com/iamsaquib8/stockwise/issues/260), [#261](https://github.com/iamsaquib8/stockwise/issues/261), [#278](https://github.com/iamsaquib8/stockwise/issues/278), [#279](https://github.com/iamsaquib8/stockwise/issues/279).

Feature freeze. Make the CLI small, the data safe and the promise written.

- 📋 **A smaller CLI** — the simplification plan ([#205](https://github.com/iamsaquib8/stockwise/issues/205)): about 25 commands in the short help,
  old names kept as hidden aliases for one release; help that fits a terminal ([#150](https://github.com/iamsaquib8/stockwise/issues/150)) and a README written for one user and
  four jobs ([#191](https://github.com/iamsaquib8/stockwise/issues/191))
- 📋 **Safe data** ([#258](https://github.com/iamsaquib8/stockwise/issues/258)) — versioned files with migrations and
  backups ([#256](https://github.com/iamsaquib8/stockwise/issues/256)), named demat accounts ([#315](https://github.com/iamsaquib8/stockwise/issues/315)) and fractional merger cash ([#317](https://github.com/iamsaquib8/stockwise/issues/317))
- 📋 **Neutral, accurate research** ([#260](https://github.com/iamsaquib8/stockwise/issues/260)) and **AI answers checked by an eval set** ([#255](https://github.com/iamsaquib8/stockwise/issues/255))
- 📋 **A written promise** — the 1.0 stability policy ([#195](https://github.com/iamsaquib8/stockwise/issues/195)) and frozen 1.0 `--json` schemas
  ([#223](https://github.com/iamsaquib8/stockwise/issues/223), [#180](https://github.com/iamsaquib8/stockwise/issues/180))
- 📋 **Quality gates** — required CI ([#171](https://github.com/iamsaquib8/stockwise/issues/171)), cargo-deny ([#172](https://github.com/iamsaquib8/stockwise/issues/172)), hardened workflows
  ([#173](https://github.com/iamsaquib8/stockwise/issues/173)), CLI snapshot tests ([#181](https://github.com/iamsaquib8/stockwise/issues/181)), parser fuzzing ([#182](https://github.com/iamsaquib8/stockwise/issues/182)), a weekly live-data canary
  ([#245](https://github.com/iamsaquib8/stockwise/issues/245))

Done when: the release candidate runs a full user journey (install, init, import, portfolio, tax,
MCP) on macOS, Linux and Windows without a known bug in the four jobs.

### 1.0 — public launch ([milestone](https://github.com/iamsaquib8/stockwise/milestone/3), target: February 2027)

Epic: [#281](https://github.com/iamsaquib8/stockwise/issues/281).

Launch before the 31 March harvesting deadline and the ITR season, when the four jobs matter most.

- 📋 **Show it** — "Open in Codespaces" ([#240](https://github.com/iamsaquib8/stockwise/issues/240)) and three
  task guides for the core jobs ([#201](https://github.com/iamsaquib8/stockwise/issues/201))
- 📋 **Launch** — Show HN ([#199](https://github.com/iamsaquib8/stockwise/issues/199)), Reddit and TradingQnA posts timed to the harvest window
  ([#200](https://github.com/iamsaquib8/stockwise/issues/200)), Product Hunt last ([#272](https://github.com/iamsaquib8/stockwise/issues/272))
- 📋 **Measure** — a launch log and before/after numbers ([#204](https://github.com/iamsaquib8/stockwise/issues/204)); targets for the 90 days after
  launch: 150 stars, 150 installs a week, 500 unique visitors per 14 days

Done when: 1.0 is tagged, the stability page is published, and the launch posts are out.

## 💡 After 1.0

Planned, but only once the four jobs are solid. Ordered roughly by demand. Items marked "idea" live in
[Discussions](https://github.com/iamsaquib8/stockwise/discussions/categories/ideas): upvote the ones you want. Most of these live in
[#282](https://github.com/iamsaquib8/stockwise/issues/282) (more markets and deeper research) and [#280](https://github.com/iamsaquib8/stockwise/issues/280) (dashboards, alerts and practice).

**Your money**
- 📋 **Net worth** — PPF, EPF, NPS, FDs and cash next to the portfolio ([#248](https://github.com/iamsaquib8/stockwise/issues/248)); SIP tracking and a
  household view
- 📋 **Indian ITR toolkit, rest** ([#264](https://github.com/iamsaquib8/stockwise/issues/264)) — Schedule FA for foreign holdings, Form 67 for US dividend tax,
  debt funds under Sec. 50AA, crypto under Sec. 115BBH, and the Income-tax Act 2025's new forms
- 📋 **Read-only broker sync** of holdings and trades instead of CSV imports ([#254](https://github.com/iamsaquib8/stockwise/issues/254)), and quotes
  from the user's own broker API ([#253](https://github.com/iamsaquib8/stockwise/issues/253))

**Everyday use**
- 📋 **Two-way Telegram bot and scheduled digests** ([#265](https://github.com/iamsaquib8/stockwise/issues/265))
- 📋 **More install and launcher options** — mise/aqua ([idea #285](https://github.com/iamsaquib8/stockwise/discussions/285)), a signed APT/RPM repository
  ([idea #286](https://github.com/iamsaquib8/stockwise/discussions/286)), a GitHub Action ([idea #287](https://github.com/iamsaquib8/stockwise/discussions/287)), a Raycast extension ([idea #288](https://github.com/iamsaquib8/stockwise/discussions/288))

**Research & market discovery**
- 📋 **Multi-source data router, snapshot cache & universal cap tiers** ([#357](https://github.com/iamsaquib8/stockwise/issues/357)) — capability routing and field merging across NSE/BSE bulk & XBRL feeds, SEC EDGAR ([#339](https://github.com/iamsaquib8/stockwise/issues/339)), AMFI, Stooq ([#340](https://github.com/iamsaquib8/stockwise/issues/340)), Frankfurter ([#341](https://github.com/iamsaquib8/stockwise/issues/341)), Finnhub/FMP and connected brokers ([#253](https://github.com/iamsaquib8/stockwise/issues/253)), a fast 500–2,000 stock snapshot cache, and SEBI-rank / FX-adjusted `Mega / Large / Mid / Small / Micro` badges (`--cap`) across every market
- 📋 **Cross-market movers, activity boards, intraday RVOL curves & hot verdicts** ([#358](https://github.com/iamsaquib8/stockwise/issues/358)) — gainers/losers across all caps and timeframes, Most Traded by value & volume, elapsed-time-normalized Volume Shockers with braille cumulative curves, Most Bought (delivery %, bulk/block deals, institutional/Form 4), In-News buzz, `markets hot` with inline 6-pillar `verdict` scores, Top Intraday, and `markets index` (`nifty50`, `sp500`, `nasdaq100`)
- 📋 **Sector sub-industries & cross-market thematic explorers** ([#359](https://github.com/iamsaquib8/stockwise/issues/359)) — 3-level Sector → Sub-Industry → Constituents drilldown across all exchanges (including UK/DE/SG), neutral RRG rotation quadrants, and curated global/local themes (`ai`, `semis`, `battery`/`ev`, `minerals`, `defence`, `clean-energy`, `datacenter`, `ems`, `cyber`, `biotech`, `fintech`, `psu`)
- 📋 **Screener queries, trading setups, quant rulebooks & global ETF explorer** ([#266](https://github.com/iamsaquib8/stockwise/issues/266), [#360](https://github.com/iamsaquib8/stockwise/issues/360)) — `stockwise screen "roce > 20 and pe < 25"` across NIFTY Midcap 150 / Smallcap 250 / 500 ([#16](https://github.com/iamsaquib8/stockwise/issues/16)) and global indices, technical setups (`volume-shocker`, `52w-breakout`, `stage2-uptrend`, `bollinger-squeeze`, `pocket-pivot`), published quant frameworks (`quality-moat`, `garp`, `magic-formula`, `canslim`, `coffee-can`, `piotroski-strong`), `compounders`, `analyst-consensus`, `sip-consistent`, `dividend-leaders`, and a dedicated Indian/US/Global **ETF explorer** (`etf`)
- 📋 **Quarterly & annual financial results collation** ([#361](https://github.com/iamsaquib8/stockwise/issues/361)) — 8–12+ quarter and 5–10 FY tables per symbol (`Revenue/NII`, `EBITDA/OPM%`, `PAT`, `EPS`, bank `GNPA/NNPA`, `YoY%`/`QoQ%`, `CFO/PAT`, beat/miss + reaction) merged from NSE XBRL, BSE, SEC EDGAR ([#339](https://github.com/iamsaquib8/stockwise/issues/339)) and global providers, plus market/portfolio earnings-season scoreboards
- 📋 **Market-wide F&O scanners & mutual fund rolling/holdings discovery** ([#362](https://github.com/iamsaquib8/stockwise/issues/362), [idea #291](https://github.com/iamsaquib8/stockwise/discussions/291)) — `options scan` (`buildup`, `iv` rank/percentile, `uoa`, `pcr`, `rollover`) and NSE F&O ban/MWPL list (`options ban`), plus MF category heatmaps, 3Y/5Y rolling return consistency, Sharpe/Sortino/downside capture, `mf compare`, direct-vs-regular cost, US/global funds, and monthly MF holdings activity (`mf holders`, `mf activity`)
- 📋 **Market mood** ([idea #290](https://github.com/iamsaquib8/stockwise/discussions/290)), **`--explain` everywhere** — `risk`, option greeks and `tax` ([#52](https://github.com/iamsaquib8/stockwise/issues/52)), and **symbol discovery** — suggestions in every command, a picker and symbol completion ([#54](https://github.com/iamsaquib8/stockwise/issues/54))

**Markets**
- 📋 **Energy and industrial metals** — platinum, palladium and copper futures ([#20](https://github.com/iamsaquib8/stockwise/issues/20)), plus global & Indian REIT/InvIT lists

**Practice and trading**
- 📋 **Daily replay challenge** with a shareable card ([idea #289](https://github.com/iamsaquib8/stockwise/discussions/289))
- 📋 **Virtual F&O trading** in the simulator, with lot sizes and margin estimates ([idea #292](https://github.com/iamsaquib8/stockwise/discussions/292))
- 🛠️ **Broker integrations verified against live accounts** ([#269](https://github.com/iamsaquib8/stockwise/issues/269), help wanted) — `trade selftest` and the
  [verification criteria](docs/brokers.md#help-verify-a-broker) are ready; we need 3 reports per
  broker plus one small live order a tester places themselves
- 📋 **SEBI retail-algo rules** in `trade` — order-rate limits, algo tagging and static-IP setup
  ([#132](https://github.com/iamsaquib8/stockwise/issues/132))
- 📋 **More brokers through OpenAlgo** — one REST adapter to the brokers it supports ([#270](https://github.com/iamsaquib8/stockwise/issues/270))

**Exploring**
- 💡 Options strategy backtests on end-of-day chains ([idea #293](https://github.com/iamsaquib8/stockwise/discussions/293))
- 💡 Trade journal and two-leg (OCO) GTTs; position sizing in `stoploss` ([#22](https://github.com/iamsaquib8/stockwise/issues/22))
- 💡 Ownership signals — shareholding and pledge changes, alerts on phrases in filings ([idea #294](https://github.com/iamsaquib8/stockwise/discussions/294))
- 💡 An assistant pane in the TUI and web dashboard ([idea #295](https://github.com/iamsaquib8/stockwise/discussions/295))
- 💡 Gulf exchanges; MCX futures and live G-sec prices via optional broker feeds ([idea #296](https://github.com/iamsaquib8/stockwise/discussions/296))
- 💡 WASM plugins for importers and data providers — kept experimental until someone uses them ([idea #297](https://github.com/iamsaquib8/stockwise/discussions/297))

### Investor intelligence, guided flows and portfolio math

Today StockWise answers dozens of separate questions (`portfolio`, `xray`, `rebalance`, `harvest`,
`tax`, `goal`, `verdict`, `score`, `insights`, `events`), but leaves the investor to stitch them
together. After 1.0's core data and CLI foundation ([#205](https://github.com/iamsaquib8/stockwise/issues/205),
[#258](https://github.com/iamsaquib8/stockwise/issues/258)) are locked in, the next frontier is
connecting those engines around **your actual portfolio, cashflows and goals** — while keeping
every output transparent, deterministic, local-first, and clear of buy/sell calls or stock tips
([#260](https://github.com/iamsaquib8/stockwise/issues/260), [DISCLAIMER.md](DISCLAIMER.md)).

**Guided flows (by investor moment, not by command)**
- 📋 **First run, `stockwise use` switcher, investor profile & smart symbols** ([#192](https://github.com/iamsaquib8/stockwise/issues/192), [#374](https://github.com/iamsaquib8/stockwise/issues/374), [#388](https://github.com/iamsaquib8/stockwise/issues/388), [#54](https://github.com/iamsaquib8/stockwise/issues/54)) — `stockwise init`, `stockwise use <market>` / `use -` / per-portfolio market & `base_currency` binding, `passive | active:low/med/high | ultra` profile modes with personal tax-slab & concentration guardrails, zero-`-m` cross-market ticker/ISIN resolution, and one-line contextual "next check" hints when a command spots high overlap, fee drag or unused tax headroom
- 💡 **Monthly payday / DCA / SIP deploy flow** ([idea #364](https://github.com/iamsaquib8/stockwise/discussions/364)) (`portfolio deploy <AMOUNT>`) — extend inflow rebalancing (`rebalance --inflow`) to route fresh monthly cash across underweight holdings, wrappers (`ISA`/`Roth IRA`/`TFSA`) and underfunded goals while respecting X-Ray single-stock caps, whole/fractional share rules and fund minimums, with zero sell orders
- 💡 **Pre-buy fit & what-if sandbox** ([idea #369](https://github.com/iamsaquib8/stockwise/discussions/369)) (`portfolio fit <SYMBOL|MF:CODE> [AMOUNT]`) — before adding a stock, ETF or fund, show the exact before → after shift in look-through overlap (`xray`), sector, currency and cap weights, portfolio beta, correlation and historical drawdown
- 📋 **Pre-sell lot, tax & exit-load check** ([#375](https://github.com/iamsaquib8/stockwise/issues/375), [#390](https://github.com/iamsaquib8/stockwise/issues/390), [idea #366](https://github.com/iamsaquib8/stockwise/discussions/366)) (`portfolio sell --preview`) — compare **`FIFO` vs `HIFO` vs `MinTax`** (US/AU), **Section 104 / Bed & Breakfast** (UK), **`ACB`** (CA) or **per-demat FIFO** (IN/DE), exact short- vs long-term tax, exit-load windows, and a **"wait N days" countdown** when a lot is close to crossing the long-term threshold or triggering a 30-day wash/superficial loss
- 💡 **March / year-end checklist** (`tax checklist`) — one guided run combining unused ₹1.25L Section 112A LTCG step-up, FIFO-aware loss harvesting (`harvest`), 8-year carry-forward loss expiry warnings, Section 234B/234C advance-tax status, unapplied corporate actions ([#95](https://github.com/iamsaquib8/stockwise/issues/95), [#96](https://github.com/iamsaquib8/stockwise/issues/96)) and broker P&L reconciliation ([#203](https://github.com/iamsaquib8/stockwise/issues/203))
- 💡 **Calm "only what changed" digest** (building on [#118](https://github.com/iamsaquib8/stockwise/issues/118), [#121](https://github.com/iamsaquib8/stockwise/issues/121) and [#265](https://github.com/iamsaquib8/stockwise/issues/265)) — post-market or Sunday briefing that surfaces only material deltas: new red flags, drift band breaches, lots turning long-term, upcoming earnings/ex-dates and goal progress

**Overall portfolio checkup & portfolio-aware analysis**
- 📋 **Holistic portfolio checkup & year-end wrap** ([#376](https://github.com/iamsaquib8/stockwise/issues/376), [#392](https://github.com/iamsaquib8/stockwise/issues/392), [idea #365](https://github.com/iamsaquib8/stockwise/discussions/365)) (`portfolio checkup` / `portfolio wrap [--redact]`) — a single, prioritized diagnostic across six lenses (*Concentration & Overlap*, *Fundamental Quality*, *Valuation Stretch*, *Downside Resilience*, *Fee, Domicile & Tax Drag*, *Goal Alignment*), ranking factual observations by `base_currency` impact (including global ETF TER bleed, non-US investor 30% US WHT / \$60k estate exposure vs UCITS, asset-location drag, and Indian Regular-vs-Direct MF bleed) alongside a shareable year-in-review scorecard
- 💡 **Portfolio-aware stock & fund lens** (`--for-portfolio` on `verdict`, `deep` and `mf`) — evaluate any candidate *relative to what you already hold*: marginal contribution to portfolio risk (MCTR), correlation with your top holdings, added X-Ray overlap, and whether it dilutes or deepens existing sector and factor tilts
- 💡 **Index Twin / Public Market Equivalent** ([idea #370](https://github.com/iamsaquib8/stockwise/discussions/370)) (`portfolio pme`) — mirror every historical buy, sell and dividend on its exact date into an index (`NIFTYBEES`, `NIFTY500`, `SPY`) or a custom blended asset benchmark (e.g. 60% equity / 30% gilt / 10% gold) to measure exact rupee value added or lost vs passive indexing
- 💡 **Household / family view** (`--household`, extending [#116](https://github.com/iamsaquib8/stockwise/issues/116), [#248](https://github.com/iamsaquib8/stockwise/issues/248), [#315](https://github.com/iamsaquib8/stockwise/issues/315)) — aggregate named portfolios across spouse, parents or HUF for combined asset allocation, emergency-fund runway, and per-PAN ₹1.25L LTCG exemption headroom

**Early red flags & forensic risk radar**
- 📋 **Portfolio-wide red-flag scan** ([#376](https://github.com/iamsaquib8/stockwise/issues/376), [idea #371](https://github.com/iamsaquib8/stockwise/discussions/371)) (`portfolio checkup` / `redflags`) — scan every held and watchlisted stock and mutual fund in one pass, ranking warnings by `position value × severity` so risks in core positions surface before small tracking bets
- 💡 **Forensic accounting & cash-flow checks** (extending `score` and `insights`) — **Beneish M-Score** (receivables bloat, gross-margin slip, asset quality, accruals), **3-year CFO-to-PAT cash conversion** (`< 0.7` flags profits not turning into cash), free-cash-flow burn, rising working-capital days, and interest-coverage (`ICR < 2x`) / short-term debt stress
- 💡 **Governance, smart-money & filing signals** (expanding [idea #294](https://github.com/iamsaquib8/stockwise/discussions/294)) — promoter holding drops, **promoter pledge levels and QoQ spikes**, simultaneous FII + DII exits, US Form 4 cluster insider selling ([#339](https://github.com/iamsaquib8/stockwise/issues/339)), and corporate-filing flags (auditor or CFO resignation, qualified opinion, credit-rating downgrade, related-party transactions, delayed results)
- 📋 **Mutual-fund silent bleed & drift** ([#375](https://github.com/iamsaquib8/stockwise/issues/375), expanding [idea #291](https://github.com/iamsaquib8/stockwise/discussions/291)) — detect **Regular vs Direct plan commission drag** in CAS imports (annual ₹ bleed, 10-year compounded cost, and which units are past exit load and 12-month LTCG for switching), TER hikes, category style drift, closet indexing (high R² with active TER) and small-cap AUM bloat
- 💡 **Thesis journal & drift guardrails** — store optional buy-thesis notes and numeric guardrails (`min_roe`, `max_pe`, `max_weight`, review date) per holding in local JSON, flagging automatically when reported numbers cross your own rules

**True diversification, correlation & factor math**
- 💡 **Effective Number of Bets** — Herfindahl-Hirschman Index (`HHI`) and `1 / HHI` on look-through X-Ray weights, plus covariance PCA **independent return factors** (*"18 stocks and 4 funds, behaving like 4.2 independent bets"*)
- 💡 **Tail / crash correlation vs calm days** (upgrading `pcorr`) — separate correlation matrices and upside/downside capture strictly on market-stress days (index down >1% or >2%), catching holdings that diversify in calm weeks but fall together in sell-offs
- 💡 **Macro & factor sensitivity** (`portfolio factors`) — rolling regression betas to Market, Size (Small/Mid vs Large), Interest Rates (India/US 10Y yields, [#19](https://github.com/iamsaquib8/stockwise/issues/19)), USD/INR ([#18](https://github.com/iamsaquib8/stockwise/issues/18) — exporters vs import-heavy cyclicals) and Gold/Crude ([#20](https://github.com/iamsaquib8/stockwise/issues/20))
- 💡 **Analytical reference weights & diversifier scan** — Minimum-Variance, Equal-Risk-Contribution (Risk Parity) and Maximum-Diversification math next to your target weights, plus screening universes ([#16](https://github.com/iamsaquib8/stockwise/issues/16), [#266](https://github.com/iamsaquib8/stockwise/issues/266)) for low-correlation candidates (`corr < 0.3` against your portfolio)

**Deep investor math & personal-finance calculators**
- 💡 **Real, post-tax, post-fee returns** — compute **post-tax, post-TER, inflation-adjusted real XIRR** alongside nominal XIRR so investors see true purchasing-power compounding
- 💡 **Reverse DCF & valuation bands** ([idea #368](https://github.com/iamsaquib8/stockwise/discussions/368)) — solve for the **market-implied 10-year FCF/EPS growth rate** baked into today's price at a chosen discount rate (*"At ₹3,900, the market prices in 13.4% annual growth"*), paired with a 2-stage DCF sensitivity grid and 5Y/10Y historical P/E and EV/EBITDA percentile bands
- 💡 **Tail risk & position-sizing math** (expanding [#22](https://github.com/iamsaquib8/stockwise/issues/22)) — Historical and Cornish-Fisher **CVaR / Expected Shortfall (95%/99%)**, Ulcer Index (drawdown depth × duration), ATR volatility-parity sizing, fractional Kelly bounds, and **portfolio heat** (% of capital lost if every stop-loss triggers at once)
- 💡 **Life-decision math** ([idea #372](https://github.com/iamsaquib8/stockwise/discussions/372)) (`goal` / `calc`) — **loan prepayment vs SIP** (effective post-tax interest saved vs post-LTCG SIP return distribution), **SWP vs FD** post-tax retirement cashflow math, **Regular-to-Direct switch breakeven** (tax paid today vs TER saved over N years), and **emergency-fund / Coast-FIRE / FI crossover** calculators

**Projections, goal extensions & crisis stress tests**
- 💡 **Net-worth-wide & Coast-FIRE goal extensions** (building on `0.9`'s `goal` overhaul in [#356](https://github.com/iamsaquib8/stockwise/issues/356)) — connect saved goals in `goals.json` to full household net worth (`PPF`, `EPF`, `NPS`, `FDs` in [#248](https://github.com/iamsaquib8/stockwise/issues/248)), Coast-FIRE / Financial-Independence crossover dates, and automatic monthly drift-to-goal alerts
- 💡 **Fat-tailed Monte Carlo & sequence-of-returns stress** (building on [#37](https://github.com/iamsaquib8/stockwise/issues/37)) — block-bootstrapped simulations from historical Indian and US return and inflation series (capturing fat tails and flat decades), plus **retirement SWP sequence-of-returns tests** (*"What if a 2008-style crash hits in Year 1 of retirement?"*) and dynamic withdrawal guardrails
- 💡 **Historical crisis replay on *your* portfolio** ([idea #367](https://github.com/iamsaquib8/stockwise/discussions/367)) (`portfolio stress`) — run 2008 GFC, 2013 taper tantrum, March 2020 COVID, 2022 rate-hike drawdown or custom multi-factor shocks (`NIFTY -25%, INR -8%, yields +150 bps`) against today's holdings to estimate rupee drawdown, biggest contributors and historical recovery time
- 💡 **Passive income & cashflow runway** — 1Y–10Y monthly cashflow projection across dividends, SGB 2.5% interest, bond coupons and FDs, showing yield-on-cost trajectory and **% of monthly expenses covered by passive income**

**Market regime, valuation percentiles & implied probabilities (base rates, not crystal balls)**
- 💡 **Valuation thermometer & forward base rates** — NIFTY 50 / Midcap 150 / Smallcap 250 / S&P 500 P/E, P/B, dividend yield and **Equity Risk Premium** (earnings yield minus 10Y sovereign yield) ranked as 10Y/20Y percentiles, paired with the **historical 1Y, 3Y and 5Y forward return distributions** recorded from similar starting valuation deciles
- 💡 **Explainable market regime model** (expanding [idea #290](https://github.com/iamsaquib8/stockwise/discussions/290)) — combine trend breadth (% of universe above 50/200 DMA, 52W highs vs lows), volatility regime (India VIX / VIX percentile and term slope), yield-curve slope (`bonds` 10Y − 91D) and rolling FII/DII cash & F&O flows into transparent regime labels (*Broad Uptrend*, *Narrow/Stretched*, *High-Vol Risk-Off*, *Capitulation*) with every weight and percentile visible
- 💡 **Options-implied probability cones & event moves** — derive 1σ/2σ forward price cones (7/30/90 days) from live option-chain IV skew, and compare **options-implied ±% earnings moves** against each stock's actual 8-quarter post-earnings history (`events`)
- 💡 **Sector earnings & margin cycle breadth** (building on [#359](https://github.com/iamsaquib8/stockwise/issues/359) and [#361](https://github.com/iamsaquib8/stockwise/issues/361)) — track revenue acceleration, operating-margin expansion/compression and institutional accumulation across sectors quarter over quarter

**Behavioral guardrails & AI co-pilot playbooks**
- 💡 **Decision journal & behavioural audit** (`portfolio journal`) — measure your own historical timing: 6M/12M performance of stocks after you sold them, panic-selling during index drawdowns, annual turnover, and cumulative tax + fee drag
- 📋 **Agentic `ask --auto`, MCP tools & playbooks** ([#378](https://github.com/iamsaquib8/stockwise/issues/378), expanding [#53](https://github.com/iamsaquib8/stockwise/issues/53) and [#261](https://github.com/iamsaquib8/stockwise/issues/261)) — multi-turn profile-aware `ask -i` / `--auto` with reversible workspace actions (`stockwise undo`), plus `checkup`, `redflags`, `fit`, `stress`, `pme`, `factors` and playbooks (*"Audit my portfolio for early red flags"*, *"Where does my ₹50k SIP close the biggest gap this month?"*, *"Stress-test my retirement corpus against a 2008 crash"*)

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
