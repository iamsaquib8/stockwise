# GEMINI.md — Antigravity & Gemini CLI Assistant Guidelines

## Workspace Guidelines
- **Ecosystem**: StockWise (Rust 1.97, multi-repo architecture).
- **Communication Style**: Concise, direct, format with clickable `file://` markdown links and GitHub-Flavored Markdown. Escape literal `$` as `\$` or in backticks to prevent KaTeX rendering issues with currency amounts.
- **Pure Rust Philosophy**: Every tool, model runner, and test is 100% pure Rust. Do not introduce Python runtime scripts.

## Key Rules
1. **Financial Strictness**: Currency calculations must use integer minor units (`i64`) or `rust_decimal::Decimal`. Never let floating-point inaccuracies cause penny drift in balances or taxes.
2. **Compliance**: Under no circumstances should the agent emit stock picks, price targets, or personalized investment advice.
3. **Repository Boundaries**: Respect public vs private repository boundaries. Do not leak proprietary data lake structures, datasets, or private tax formulas into the public `stockwise` repository.

## Commands
```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```
