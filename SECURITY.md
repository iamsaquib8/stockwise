# Security Policy

StockWise can hold broker credentials and place real-money trades, so security
reports are taken seriously.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Use GitHub's private vulnerability reporting:
[**Report a vulnerability**](https://github.com/iamsaquib8/stockwise/security/advisories/new).
If that form is unavailable, email the maintainer privately at
[saquibulhassan6@gmail.com](mailto:saquibulhassan6@gmail.com) (the contact
published in `Cargo.toml`). Do not include credentials or other secrets in a
public issue.

Include reproduction steps, affected version/commit, and impact. We aim to
acknowledge within a few days and to coordinate a fix and disclosure timeline
with you.

## Scope — please pay special attention to

- Broker credentials and API keys for **Angel One SmartAPI, Zerodha Kite,
  Upstox, Dhan and Fyers**; the OS keyring and the owner-only credential-file
  fallback. See [broker setup and order safeguards](docs/brokers.md).
- The **`trade`** command's live-order boundary (`place_live_order`), including
  placing, modifying or cancelling orders, `enable_orders`, `max_order_value`,
  dry runs and confirmation checks. The **daemon is paper-only**: report
  anything that makes it reach a broker.
- Local portfolio, watchlist, notification and configuration data, including
  notification tokens/passwords supplied through environment variables or
  `[notify]` in `config.toml`.
- The read-only **`serve` API** and **`mcp --http`** network surfaces. Both bind
  to loopback by default; exposing them with `--allow-remote` requires
  `STOCKWISE_API_TOKEN` and relies on Host/Origin checks. They use **plain
  HTTP**: do not expose them unprotected to the public internet. See
  [server security](docs/server.md) and [AI/MCP setup](docs/ai.md).
- The web dashboard, including its Content Security Policy (CSP), bearer-token
  handling and rendering of API data as text rather than executable markup.
- Data sent by **`ask`**, **`report`** and **`--ai`** to opt-in cloud AI
  providers (OpenAI, Anthropic or Google Gemini). Ollama is local by default;
  review prompts and portfolio data before selecting a hosted provider. See
  [AI integration](docs/ai.md).
- The WASM plugin sandbox (restricted imports, execution-fuel budget and
  memory limits), the `install.sh` installer and published release artifacts.

## Good security hygiene for users

- Never commit `config`, credentials, or exported portfolio data.
- Keep broker secrets and notification tokens out of logs and shared files;
  restrict permissions on local credential/configuration files.
- Prefer paper trading (`sim`) and `trade --dry-run` before explicitly enabling
  live **`trade`** orders. The daemon is paper-only and cannot place live orders.
- Keep `serve` and `mcp --http` on loopback unless you have configured the API
  token and a trusted, encrypted reverse proxy; see [docs/server.md](docs/server.md).
- Use the local AI provider when portfolio data must remain on your machine;
  see [docs/ai.md](docs/ai.md) before opting in to a cloud model.

## Supported versions

This is a pre-1.0 project. Security fixes target the most recent **tagged
release** (`v0.8.1` as of this update); older release tags are not maintained
unless explicitly announced. Check the
[releases](https://github.com/iamsaquib8/stockwise/releases) for the current
supported tag.
