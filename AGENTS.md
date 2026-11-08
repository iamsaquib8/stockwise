# AGENTS.md — Master AI Instructions for StockWise

> Applicable to: **OpenAI Codex**, **ChatGPT**, **GitHub Copilot**, **DeepSeek**, **Moonshot / Kimi**, and any agentic coding harness.

---

## 1. Identity & Operating Context

You are working on **StockWise** — an institutional-grade, local-first financial terminal, quantitative backtest engine, multi-currency portfolio tracker, and AI terminal platform built in **100% pure Rust 1.97**.

The codebase includes:
- Command-line interface (`cli`)
- Interactive Terminal UI (`tui`)
- Model Context Protocol server (`mcp`)
- Background live data worker & OS service (`worker`)
- Multi-currency portfolio accounting & tax engines (`portfolio`, `tax`)

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

## 3. Mandatory Compliance & Non-Advisory Rules

- **Zero Stock Tips**: Never generate or recommend specific buy/sell decisions, price targets, or claims of guaranteed returns.
- **Compliance Wording**: Always frame outputs as *historical observations*, *tax calculations*, *diagnostic checkups*, or *empirical backtests*.
- If a user asks *"Should I buy NVDA?"*, refuse the advisory request and instead show them how to run neutral diagnostic commands:
  ```bash
  stockwise quote NVDA
  stockwise verdict NVDA
  stockwise checkup --for-holding NVDA
  ```

---

## 4. Architecture & Module Structure

| Component | Responsibility & Path |
| :--- | :--- |
| CLI commands & arguments | `src/cli/` |
| Exchange quotes & live data | `src/data/`, `src/providers/` |
| Broker CSV & JSON importers | `src/brokers/`, `src/importers/` |
| Technical analysis & indicators | `src/analysis/`, `src/indicators/` |
| Portfolio, lots & accounting | `src/portfolio/` |
| Tax calculations & workpapers | `src/tax/` |
| MCP tools & protocol | `src/mcp/` |
| Local database & caching | `src/db/`, `src/cache/` |

---

## 5. Verification Commands

Before concluding any work, always verify formatting, lints, and tests:
```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```
