# AGENTS.md — Master AI Instructions for StockWise

> Applicable to: **OpenAI Codex**, **ChatGPT**, **GitHub Copilot**, **DeepSeek**, **Moonshot / Kimi**, and any agentic coding harness.

---

## 1. Identity & Operating Context

You are working on **StockWise** — an institutional-grade, local-first financial terminal, quantitative backtest engine, multi-currency portfolio tracker, and AI terminal platform built in **100% pure Rust 1.97**.

The codebase includes:
- Command-line interface (`src/main.rs`, `src/commands/`)
- Interactive Terminal UI (`src/tui/`)
- Model Context Protocol server (`src/mcp.rs`, `src/tools/`)
- Background live data worker & OS service (`src/worker.rs`)
- Multi-currency portfolio accounting & tax engines (`src/portfolio.rs`, `src/tax.rs`)
- Rule-based actions (`src/action.rs`)

---

## 2. Core Engineering Standards

### 2.1 Pure Rust 1.97 Strictness
- **Zero Python Runtime Dependencies**: Never add Python scripts or runtimes to user-facing or inference paths.
- **Error Handling**: Use `anyhow::Result` with detailed `.with_context(|| ...)` in application CLIs; use explicit typed error enums (`thiserror`) in reusable library modules.
- **Concurrency**: Use `tokio` for async I/O and network operations; use `rayon` for CPU-bound universe backtests, factor calculations, and Monte Carlo simulations.
- **Serialization**: Every analytical command must support `--json` and `--csv` serialization with stable schemas.

### 2.2 Financial Arithmetic & Precision
- **Never use standard floating-point arithmetic for currency or cash balances**:
  - Use `rust_decimal::Decimal` or integer minor units (`i64` cents/pence/paise) for cash balances, tax calculations, and broker transactions.
  - When calculating ratios (XIRR, TTWROR, Sharpe, volatility), use `f64` with explicit clamping (`clamp()`) and `Option<f64>` for missing or warmup periods.
- **Penny Conservation**: In splits, RSU workday apportionments, and tax allocations, use integer remainder distribution so that $\sum \text{parts} \equiv \text{total}$ down to the exact minor unit.

---

## 3. Recommendations Policy

StockWise ships rule-based actions by default: BUY / WATCH / AVOID for a stock not held, ADD / HOLD / TRIM / EXIT for a holding, and NO CALL when the data is too thin.

- **One source**: Actions are rule-based and deterministic, computed only by `src/action.rs`. No other module, prompt or tool decides an action.
- **Always complete**: Every action carries its reasons (at most 3), a confidence level, an invalidation price (BUY, ADD, HOLD, TRIM), a review date and the disclaimer.
- **No guessing**: NO CALL when data coverage is below 60%.
- **Published rules**: The rules live in `docs/methodology.md` ("Actions"). Any rule or threshold change bumps `METHODOLOGY` in `src/action.rs`.
- **AI explains, never decides**: The `ask`/`ai` layer, MCP hosts and coding agents explain the computed action. They never invent, upgrade or change one, and never invent price levels (entry, stop, target, invalidation).
- **Disclaimer**: Rule-based educational signals. StockWise is not a SEBI-registered investment adviser or research analyst. Losses are possible. See `DISCLAIMER.md`.
- **Tests**: Output that can show an action is checked with `crate::action::testing::assert_action_well_formed`.
- If a user asks *"Should I buy NVDA?"*, run the engine and present its action with its reasons, invalidation price, review date and disclaimer:
  ```bash
  stockwise verdict NVDA -m us
  stockwise portfolio review
  ```

---

## 4. Architecture & Module Structure

| Component | Responsibility & Path |
| :--- | :--- |
| CLI entry point & clap definitions | `src/main.rs` |
| Command handlers | `src/commands/*.rs` |
| Rule-based actions | `src/action.rs` |
| 6-pillar score, picks | `src/verdict.rs`, `src/picks.rs` |
| Data sources & coverage | `src/provenance.rs`, `src/providers/` |
| Broker connections | `src/brokers/` |
| Portfolio, lots & accounting | `src/portfolio.rs` |
| Tax calculations | `src/tax.rs` |
| MCP server & tools | `src/mcp.rs`, `src/mcp/`, `src/tools/` |
| Terminal UI | `src/tui/` |
| Local SQLite cache & background worker | `src/db.rs`, `src/worker.rs` |

---

## 5. Verification Commands

Before concluding any work, always verify formatting, lints, and tests:
```bash
cargo +stable fmt --all -- --check
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test
```
