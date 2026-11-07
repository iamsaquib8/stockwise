<p align="center">
  <img src="assets/banner.svg" alt="StockWise — stock market toolkit for your terminal" width="760">
</p>

# StockWise

**A stock market toolkit for your terminal, built for Indian (NSE/BSE) and US
stocks.** Quotes, charts, screeners, mutual funds, options, commodities,
backtests, paper trading, portfolio tracking, capital-gains tax and an MCP
server for AI assistants — in one fast binary. No API keys, no account.

## Install

```bash
# macOS / Linux / Termux on Android
curl -fsSL https://raw.githubusercontent.com/iamsaquib8/stockwise/main/install.sh | sh

# Debian/Ubuntu, Fedora/RHEL: .deb and .rpm from the latest release
sudo dnf install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise-x86_64.rpm

# With Rust (1.97+)
cargo binstall stockwise            # prebuilt binary
cargo install --locked stockwise    # build from source
```

Windows: `scoop install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise.json`
or the `.zip` from the
[latest release](https://github.com/iamsaquib8/stockwise/releases/latest).
Docker: `docker run --rm ghcr.io/iamsaquib8/stockwise quote RELIANCE`.
Nix, `.deb`/`.rpm` and what's coming next (PyPI, Homebrew, AUR, winget): [Install](install.md).

Then run `stockwise doctor` to check your setup.

## Thirty seconds

```bash
stockwise quote RELIANCE TCS            # Indian stocks are the default
stockwise quote AAPL NVDA -m us         # US stocks
stockwise deep HDFCBANK                 # everything about one stock
stockwise mf sip 122639 5000 -p 5y      # what a ₹5,000/month SIP became
stockwise backtest INFY -s macd         # a strategy after STT and brokerage
stockwise sim replay start covid-crash-2020   # trade the 2020 crash, future hidden
stockwise portfolio xray                # your real exposure across stocks, funds and ETFs
stockwise tax advance                   # advance-tax instalments and Sec. 234C interest
```

<p align="center">
  <img src="assets/history.svg" alt="stockwise history AAPL -m us -p 6mo" width="720">
</p>

## Where to next

- **[Cheatsheet](cheatsheet.md)** — the commands people use most, on one page.
- **[Commands](commands.md)** — every command and option.
- **[Markets](markets.md)** — India, US, global exchanges, crypto, FX, commodities.
- **[Simulator](simulator.md)** — practise on historical crashes and earn badges.
- **[Brokers](brokers.md)** — optional live orders, always behind a preview.

StockWise is open source under the MIT licence:
[github.com/iamsaquib8/stockwise](https://github.com/iamsaquib8/stockwise).
It is for education and information, **not financial advice** — see the
[disclaimer](disclaimer.md).
