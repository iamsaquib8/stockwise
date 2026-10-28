# Contributing to StockWise

Thanks for your interest! Contributions of all sizes are welcome — bug reports,
docs, new commands, broker integrations, and strategy ideas.

## Ground rules

- Be kind. See the [Code of Conduct](CODE_OF_CONDUCT.md).
- StockWise is a financial tool. Read the [Disclaimer](DISCLAIMER.md) — never
  hard-code real broker credentials, API keys, or personal portfolio data into
  the repo, tests, or fixtures.

## Where to start

Issues labelled [good first issue](https://github.com/iamsaquib8/stockwise/labels/good%20first%20issue) are small and
self-contained: each says what to change, links the code, and lists what
"done" looks like. Comment on one to claim it so two people don't work on the
same thing. Larger ideas are on the [roadmap](ROADMAP.md); open a discussion or
issue before starting one.

## Getting set up

```bash
git clone https://github.com/iamsaquib8/stockwise
cd stockwise
cargo build
cargo test
```

Requires Rust **1.97+** (edition 2024). We support the latest stable release and the two before it. Optional at runtime: a local
[Ollama](https://ollama.com) server for AI features and Redis for caching — both
degrade gracefully when absent.

Try your build without touching your real portfolio by pointing StockWise at a
scratch data directory:

```bash
export STOCKWISE_HOME=$(mktemp -d)
cargo run -- doctor
cargo run -- quote RELIANCE TCS
cargo run -- portfolio add TCS 10 3500 && cargo run -- portfolio
```

## Finding your way around

[docs/architecture.md](docs/architecture.md) maps the source tree. In short:
`src/main.rs` defines the CLI, `src/commands/` holds one file per command area
(`overview`, `technicals`, `markets`, `holdings`, `trade`, ...), and the
analysis engines (`technical.rs`, `backtest.rs`, `intraday.rs`, `longterm.rs`)
are pure functions with unit tests.

## Before you open a PR

Run the same checks CI does — all of them must pass:

```bash
cargo fmt --all            # format
cargo clippy --all-targets -- -D warnings   # lint (must be clean)
cargo clippy --all-targets --no-default-features -- -D warnings   # minimal build (no tui/serve/mcp)
cargo clippy --all-targets --features wasm-plugins -- -D warnings  # WASM strategy plugins
cargo test                 # all tests green
cargo test --features wasm-plugins
```

- Keep PRs focused; one logical change per PR.
- Add tests for new behavior — CLI parsing tests live in `src/main.rs`, pure logic tests next to the code.
- Update `docs/`, `README.md`, and `CHANGELOG.md` (`[Unreleased]`) when behavior
  changes.
- New commands: add the variant to `Commands` and to the `GROUPS` help table in
  `src/main.rs` (a test checks every command is grouped), write `cmd_<name>` in
  the matching `src/commands/*.rs` file, and document it in `docs/commands.md`.
- Errors: return `anyhow` errors (`bail!`) instead of printing and returning
  `Ok(())`, so scripts see a non-zero exit code. Empty states (nothing in the
  portfolio yet) are not errors.
- Anything that can place a live order or GTT must go through `place_live_order`
  in `src/commands/trade.rs` — never call `Broker::place_order` or
  `Broker::gtt_create` directly.

## Good first contributions

- `--json` / `--csv` output for another command (`analyze`, `peers`, `news`,
  `picks`, ...) — build rows once and render them with `src/output.rs`; see
  `cmd_portfolio_show` or `cmd_risk` for the pattern, and add the shape to
  `docs/commands.md`.
- More symbols in the India/US universes in `src/market.rs`, or a refresh when
  a ticker changes.
- Unit tests for an indicator in `src/technical.rs` against a known reference.
- Docs: a worked example for a command you use.

## Adding a data source or broker

Network access goes through `src/api.rs` (rate limiting, retry, caching). Broker
integrations implement the `Broker` trait in `src/brokers/` — see `zerodha.rs`
for a compact example. Keep secrets in `secrets::Secret` fields (stored in the OS
keyring via `src/brokers/secrets.rs`), never in source, logs or error messages.
Test against `brokers::mock::MockServer` with fixtures from the broker's docs, and
label anything not yet run against a live account "experimental". Give each read
endpoint a `Shape` (the fields the parser reads and the documented ones) and check
your fixtures with `mock::assert_fixture_shapes`, so `trade selftest` can tell
testers when a live response differs. Anything that only reads from a real account
(`trade selftest`, `trade sync`) must run on a `brokers::readonly::ReadOnly` session.

## Commit messages

Conventional-ish prefixes keep the changelog easy: `feat:`, `fix:`, `docs:`,
`test:`, `refactor:`, `chore:`.

## Reporting bugs / requesting features

Use the [issue templates](https://github.com/iamsaquib8/stockwise/issues/new/choose).
For open-ended ideas, start a
[Discussion](https://github.com/iamsaquib8/stockwise/discussions).
