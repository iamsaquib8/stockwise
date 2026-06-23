# Security Policy

StockWise can hold broker credentials and place real-money trades, so security
reports are taken seriously.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Use GitHub's private vulnerability reporting:
[**Report a vulnerability**](https://github.com/iamsaquib8/stockwise/security/advisories/new).

Include reproduction steps, affected version/commit, and impact. We aim to
acknowledge within a few days and to coordinate a fix and disclosure timeline
with you.

## Scope — please pay special attention to

- Handling of broker credentials and API keys (Angel One and any future broker).
- Anything that could place, modify, or cancel orders unexpectedly.
- Local storage of portfolio, watchlist, and config files.
- The daemon's risk controls (per-trade size, daily loss kill switch, max
  positions).

## Good security hygiene for users

- Never commit `config`, credentials, or exported portfolio data.
- Prefer paper trading (`sim`) before enabling live `trade`/`daemon` flows.
- Review the risk controls in [docs/bot-design.md](docs/bot-design.md) before
  running a daemon with real capital.

## Supported versions

This is a pre-1.0 project; only the latest release on the default branch
receives security fixes.
