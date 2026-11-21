# GEMINI.md — Antigravity & Gemini CLI Assistant Guidelines

## Workspace Guidelines
- **Ecosystem**: StockWise (Rust 1.97, modular architecture).
- **Communication Style**: Concise, direct, format with clickable `file://` markdown links and GitHub-Flavored Markdown. Escape literal `$` as `\$` or in backticks to prevent KaTeX rendering issues with currency amounts.
- **Pure Rust Philosophy**: Every tool, model runner, and test is 100% pure Rust. Do not introduce Python runtime scripts.

## Key Rules
1. **Financial Strictness**: Currency calculations must use integer minor units (`i64`) or `rust_decimal::Decimal`. Never let floating-point inaccuracies cause penny drift in balances or taxes.
2. **Recommendations policy**: Actions (BUY/WATCH/AVOID, ADD/HOLD/TRIM/EXIT, NO CALL) are rule-based and deterministic, computed only by `src/action.rs`. Every action carries at most 3 reasons, a confidence level, an invalidation price (BUY, ADD, HOLD, TRIM), a review date and the disclaimer; NO CALL below 60% data coverage. The rules live in `docs/methodology.md` ("Actions"), and any rule or threshold change bumps `METHODOLOGY`. The agent explains the computed action and never invents, upgrades or changes one, and never invents price levels. StockWise is not a SEBI-registered investment adviser or research analyst (`DISCLAIMER.md`).
3. **Clean Open-Source Hygiene**: Maintain clean open-source repository boundaries. Zero raw exchange data, zero hardcoded secrets.

## Commands
```bash
cargo +stable fmt --all -- --check
cargo +stable clippy --all-targets --all-features -- -D warnings
cargo +stable test
```
