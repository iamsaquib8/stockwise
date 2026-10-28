# StockWise

**A stock market toolkit for your terminal, built for Indian (NSE/BSE) and US
stocks.** Quotes, charts, screeners, backtests, SIP math, paper trading and
portfolio tracking in one fast binary. No API keys, no account.

This image is a mirror of `ghcr.io/iamsaquib8/stockwise`: the same
multi-arch image (linux/amd64, linux/arm64), built from the
[Dockerfile](https://github.com/iamsaquib8/stockwise/blob/master/Dockerfile)
on every release.

```bash
docker run --rm -it -v stockwise-data:/data iamsaquib8/stockwise quote RELIANCE TCS
docker run --rm -it -v stockwise-data:/data iamsaquib8/stockwise backtest INFY -s macd
docker run --rm -it -v stockwise-data:/data iamsaquib8/stockwise tui
```

- Portfolio, watchlist, alerts and history live in `/data`; mount a volume to
  keep them.
- Runs as a non-root user on a distroless base (no shell).
- Tags: `latest`, `<major>.<minor>` and the exact version, e.g. `0.5.0`.
- The web dashboard and JSON API listen outside the container only with a
  bearer token:
  `docker run --rm -p 8787:8787 -e STOCKWISE_API_TOKEN=<secret> -v stockwise-data:/data iamsaquib8/stockwise serve --bind 0.0.0.0:8787 --allow-remote`

Documentation: <https://iamsaquib8.github.io/stockwise/> ·
Source: <https://github.com/iamsaquib8/stockwise> · MIT licensed.

> **Not financial advice.** StockWise is for education and information. See the
> [disclaimer](https://github.com/iamsaquib8/stockwise/blob/master/DISCLAIMER.md).
