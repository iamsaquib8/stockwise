# CLAUDE.md — Context and Guidelines for Claude Code

## Project Overview
StockWise is a local-first financial terminal, quantitative backtest engine, and MCP server ecosystem written in **100% Pure Rust 1.97**.

It spans 5 repositories:
1. `iamsaquib8/stockwise`: Public core CLI (`src/main.rs`, `src/cli/`, `src/portfolio/`, `src/tax/`).
2. `iamsaquib8/stockwise-pool`: Private point-in-time quant research lake, CPCV runner, and factor engine.
3. `iamsaquib8/stockwise-slm`: Private in-house financial SLM fine-tuning and GGUF/ONNX packaging.
4. `iamsaquib8/stockwise-pro`: Private statutory tax filing packs and CPA/CA audit workpapers.
5. `iamsaquib8/stockwise-hub`: Private offline-first Ed25519 licensing and default AI gateway.

## Build and Test Commands
```bash
# Check formatting
cargo fmt --all -- --check

# Run linter with strict warnings
cargo clippy --all-targets -- -D warnings

# Run all unit and integration tests
cargo test

# Run a specific test
cargo test test_name -- --nocapture

# Build release binary
cargo build --release
```

## Architecture & Code Conventions
- **Language**: Pure Rust 1.97 (2024 edition). No Python runtime dependencies.
- **Error Handling**: Use `anyhow::Result` in application binaries; `thiserror` in libraries.
- **Financial Math**: Integer minor units (`i64` cents/pence/paise) or `rust_decimal::Decimal` for money. `f64` for analytical ratios (XIRR, Sharpe, beta).
- **Compliance**: Never output financial advice, stock picks, or price targets (`DISCLAIMER.md`).
- **Data Protection**: Zero raw market-data leakage; never commit raw OHLCV series.
- **Offline First**: All core commands operate 100% offline from local files; Pro features verify offline Ed25519 signatures without runtime telemetry.
