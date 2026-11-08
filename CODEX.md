# CODEX.md — OpenAI Codex & ChatGPT Instructions for StockWise

See [AGENTS.md](AGENTS.md) for complete multi-repo architecture, Rust engineering rules, compliance boundaries, and financial arithmetic standards.

### Quick Rules for Codex:
1. **Target**: Pure Rust 1.97 (`cargo fmt`, `cargo clippy`, `cargo test`).
2. **Precision**: Currency in integer minor units (`i64`) or `rust_decimal::Decimal`.
3. **Compliance**: No buy/sell recommendations or price targets.
4. **Data Isolation**: Never hardcode or leak private repository logic or raw market series into public `iamsaquib8/stockwise`.
