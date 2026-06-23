# Contributing to StockWise

Thanks for your interest! Contributions of all sizes are welcome — bug reports,
docs, new commands, broker integrations, and strategy ideas.

## Ground rules

- Be kind. See the [Code of Conduct](CODE_OF_CONDUCT.md).
- StockWise is a financial tool. Read the [Disclaimer](DISCLAIMER.md) — never
  hard-code real broker credentials, API keys, or personal portfolio data into
  the repo, tests, or fixtures.

## Getting set up

```bash
git clone https://github.com/iamsaquib8/stockwise
cd stockwise
cargo build
cargo test
```

Requires Rust **1.88+** (edition 2024 + let-chains). Optional at runtime: a local
[Ollama](https://ollama.com) server for AI features and Redis for caching — both
degrade gracefully when absent.

## Before you open a PR

Run the same checks CI does — all three must pass:

```bash
cargo fmt --all            # format
cargo clippy --all-targets -- -D warnings   # lint (must be clean)
cargo test                 # all tests green
```

- Keep PRs focused; one logical change per PR.
- Add tests for new behavior. We have 213 and would like more.
- Update `docs/`, `README.md`, and `CHANGELOG.md` (`[Unreleased]`) when behavior
  changes.
- New commands: wire them into `commands.rs`, document them in
  `docs/commands.md`, and add to the README command table.

## Adding a data source or broker

Network access goes through `src/api.rs` (rate limiting, retry, caching). Broker
integrations live alongside `src/angel.rs`. Keep secrets in config/keyring, never
in source. See the [ROADMAP](ROADMAP.md) for the brokers we'd love help with.

## Commit messages

Conventional-ish prefixes keep the changelog easy: `feat:`, `fix:`, `docs:`,
`test:`, `refactor:`, `chore:`.

## Reporting bugs / requesting features

Use the [issue templates](https://github.com/iamsaquib8/stockwise/issues/new/choose).
For open-ended ideas, start a
[Discussion](https://github.com/iamsaquib8/stockwise/discussions).
