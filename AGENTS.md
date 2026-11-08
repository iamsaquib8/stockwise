# AGENTS.md — Master AI Instructions for StockWise Ecosystem

> Applicable to: **OpenAI Codex**, **ChatGPT**, **GitHub Copilot**, **DeepSeek**, **Moonshot / Kimi**, and any agentic coding harness.

---

## 1. Identity & Operating Context

You are working on the **StockWise** financial ecosystem — an institutional-grade, local-first quantitative research, multi-currency portfolio tracking, and AI terminal platform built in **100% pure Rust 1.97**.

The ecosystem consists of:
1. `iamsaquib8/stockwise` (Public MIT): The core binary (`cli`, `tui`, `mcp`, `worker`).
2. `iamsaquib8/stockwise-pool` (Private): The point-in-time quant research data lake, CPCV backtester, and empirical Base-Rate Atlas compiler.
3. `iamsaquib8/stockwise-slm` (Private): In-house Small Language Models, fine-tuning datasets, and GGUF/ONNX/vLLM release pipelines.
4. `iamsaquib8/stockwise-pro` (Private): Statutory tax form compilers (IRS 8949, HMRC SA108, CRA T1135, India Schedule FA), DTAA matrix, and audit workpapers.
5. `iamsaquib8/stockwise-hub` (Private): Offline-first Ed25519 license issuer, Cursor-style default AI gateway (`ai.stockwise.dev`), and zero-knowledge backup relay.

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

## 4. Multi-Repo Boundary Guidelines

| Task | Correct Target Repo |
| :--- | :--- |
| Adding a broker CSV importer or exchange quote provider | `stockwise` (`src/brokers/` or `src/importers/`) |
| Adding a technical indicator or CLI command | `stockwise` (`src/cli/` or `src/analysis/`) |
| Adding historical index constituent tracking or CPCV backtest math | `stockwise-pool` (`schema/` or `src/metrics.rs`) |
| Fine-tuning an SLM or creating tool-calling dataset trajectories | `stockwise-slm` (`datasets/` or `configs/`) |
| Implementing a statutory tax form (IRS 8949, HMRC SA108, Schedule FA) | `stockwise-pro` (`packs/` or `src/`) |
| Implementing offline Ed25519 licensing or backup encryption | `stockwise-hub` (`src/`) |

---

## 5. Verification Commands

Before concluding any work, always verify formatting, lints, and tests:
```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```
