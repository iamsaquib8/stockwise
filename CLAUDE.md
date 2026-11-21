# CLAUDE.md — Context and Guidelines for Claude Code

## Project Overview
StockWise is a local-first market toolkit for Indian and US investors: a CLI
and TUI, a portfolio and tax engine, a backtester and an MCP server, written in
**pure Rust 1.97** (2024 edition).

## Layout
- `src/main.rs`: CLI entry point and the clap definitions.
- `src/commands/*.rs`: command handlers.
- `src/portfolio.rs`, `src/tax.rs`: portfolio accounting, lots and capital-gains tax.
- `src/action.rs`: rule-based actions (BUY/WATCH/AVOID, ADD/HOLD/TRIM/EXIT, NO CALL).
- `src/verdict.rs`: the 6-pillar score. `src/picks.rs`: ranked picks.
- `src/provenance.rs`: data sources and coverage.
- `src/mcp.rs`, `src/mcp/`, `src/tools/`: MCP server and its tools.
- `src/tui/`: terminal UI. `src/db.rs`: local SQLite cache. `src/worker.rs`: background worker.

## Build and Test Commands
```bash
cargo +stable fmt --all -- --check
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test
cargo +stable test test_name -- --nocapture   # one test
cargo +stable build --release
```

## Conventions
- **Errors**: `anyhow::Result` in the binary; `thiserror` in library modules.
- **Money**: integer minor units (`i64` paise/cents/pence) or `rust_decimal::Decimal`.
  `f64` only for analytical ratios (XIRR, Sharpe, beta).
- **Data**: never commit raw OHLCV series outside test fixtures.
- **Network and privacy**: StockWise fetches market data at run time (Yahoo,
  NSE/BSE, EDGAR, and optional keyed providers). It has no telemetry and keeps
  all user data local (portfolio, notes, cache in the data directory). Many
  commands (portfolio math, tax, backup) work offline from local files; keep
  them that way.
- **Changelog**: every user-visible change adds a fragment in `changelog.d/`
  (see `changelog.d/README.md`); do not edit `CHANGELOG.md` directly. Check with
  `python3 scripts/changelog.py check`.

## Recommendations policy
- Actions are rule-based and deterministic, computed only by `src/action.rs`.
- Every action carries its reasons (at most 3), a confidence level, a review
  date and the disclaimer; BUY, ADD, HOLD and TRIM also carry the invalidation
  price (a daily close below it invalidates the action).
- NO CALL when data coverage is below 60%; never guess.
- The rules live in `docs/methodology.md` ("Actions"). Any rule or threshold
  change bumps `METHODOLOGY` in `src/action.rs`.
- AI (the `ask`/`ai` layer, MCP hosts, coding agents) explains the computed
  action. It never invents, upgrades or changes one, and never invents price
  levels.
- Disclaimer: rule-based educational signals; StockWise is not a
  SEBI-registered investment adviser or research analyst; losses are possible
  (`DISCLAIMER.md`).
- Tests that print actions use `crate::action::testing::assert_action_well_formed`.
