# CODEX.md — OpenAI Codex & ChatGPT Instructions for StockWise

See [AGENTS.md](AGENTS.md) for complete architecture, Rust engineering rules, the recommendations policy, and financial arithmetic standards.

### Quick Rules for Codex:
1. **Target**: Pure Rust 1.97 (`cargo +stable fmt`, `cargo +stable clippy --all-targets --all-features -- -D warnings`, `cargo +stable test`).
2. **Precision**: Currency in integer minor units (`i64`) or `rust_decimal::Decimal`.
3. **Recommendations policy**: Actions (BUY/WATCH/AVOID, ADD/HOLD/TRIM/EXIT, NO CALL) are rule-based and deterministic, computed only by `src/action.rs`. Each carries at most 3 reasons, a confidence level, an invalidation price (BUY, ADD, HOLD, TRIM), a review date and the disclaimer. NO CALL below 60% data coverage. Rules live in `docs/methodology.md` ("Actions"); any rule change bumps `METHODOLOGY`. AI explains the computed action and never invents, upgrades or changes one, or invents price levels. Tests use `crate::action::testing::assert_action_well_formed`.
4. **Data Isolation**: Never commit raw exchange bar series or hardcode proprietary keys.
