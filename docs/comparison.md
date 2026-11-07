# How StockWise compares

StockWise is a free, open-source command-line toolkit that runs on your own
machine. This page compares it with tools people often use instead, so you can
pick the right one. It is based on each project's public documentation as of
October 2026, and the StockWise column was checked against the code in November
2026. If something here is out of date,
[open an issue](https://github.com/iamsaquib8/stockwise/issues) and we'll fix it.

Key: ✓ yes · ◐ partly · ✗ no

## Open-source tools

| | StockWise | OpenBB | ticker | Ghostfolio | Wealthfolio | Portfolio Performance | Freqtrade | OpenAlgo |
|---|---|---|---|---|---|---|---|---|
| Runs as | CLI, TUI, local web | Python, API | TUI | Self-hosted web | Desktop app | Desktop app | Bot + web UI | Self-hosted web |
| Indian stocks (NSE/BSE) | ✓ default | ◐ | ◐ | ◐ | ◐ | ◐ | ✗ | ✓ via brokers |
| US and global stocks | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | ◐ |
| Indian mutual funds (AMFI) | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Indian capital-gains tax, ITR-style export | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Portfolio with lots and XIRR | ✓ | ✗ | ◐ | ✓ | ✓ | ✓ | ✗ | ✗ |
| Option chains, greeks, payoff | ✓ NSE, BSE, US | ✓ US | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |
| Backtesting with Indian costs | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ◐ crypto | ◐ |
| Paper trading and historical replay | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ dry-run | ✓ sandbox |
| Live broker orders | ◐ Angel One; 4 experimental | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ crypto | ✓ many brokers |
| Scripting (`--json`, REST) | ✓ | ✓ | ✗ | ◐ | ◐ | ✗ | ✓ | ✓ |
| No account or API key needed | ✓ | ◐ | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ |
| Single binary, no runtime | ✓ | ✗ | ✓ | ✗ | ✓ | ✗ | ✗ | ✗ |

## Web and app platforms

Popular Indian platforms such as Screener.in, Trendlyne, Tickertape,
StockEdge, Sensibull, Zerodha Console and Kuvera go deeper in their own areas —
fundamentals and screening data, open-interest analytics, family accounts —
and some of that depth relies on licensed data that a free tool can't
redistribute. StockWise's angle is different:

- **Your data stays on your machine.** Holdings live in local files, broker
  secrets in your OS keyring, and AI runs locally by default.
- **One tool across the whole journey** — research, funds, options, tax,
  backtests, practice and orders — instead of a separate app for each.
- **Scriptable.** Many commands take `--json` and tables take `--csv`, and
  `stockwise serve` exposes a local API.
- **Free and open source** (MIT).

## When to use something else

- You want the deepest fundamentals and custom screening on Indian companies:
  a dedicated screener such as Screener.in.
- You trade options daily and need intraday open-interest charts or a strategy
  builder with live P&L: a dedicated options platform. `options oi` is a
  snapshot of one expiry.
- You want a polished desktop or phone app for a household's net worth:
  Wealthfolio, Ghostfolio or your broker's app.
- You run 24/7 crypto trading bots: Freqtrade.
- You want broker connectivity across dozens of Indian brokers: OpenAlgo.

StockWise is for education and information, not financial advice — see the
[disclaimer](../DISCLAIMER.md).
