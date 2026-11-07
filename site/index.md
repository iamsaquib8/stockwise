<p align="center">
  <img src="assets/banner.svg" alt="StockWise — stock market toolkit for your terminal" width="760">
</p>

# StockWise

**A command-line tool for investors in Indian (NSE/BSE) and US markets.** It tracks
your portfolio with exact lots and XIRR, works out Indian capital-gains tax and
advance tax, looks up stocks, mutual funds and options, and lets an AI assistant
read your portfolio through an MCP server. Your data stays in local files. One
binary, no account, no API key.

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
Nix and the `.deb`/`.rpm` details are on the [Install](install.md) page, which also lists
what is not published yet (PyPI, Homebrew, winget, AUR).

Then run `stockwise doctor` to check your setup. If a command fails, see
[Troubleshooting](troubleshooting.md).

## Thirty seconds

```bash
stockwise quote RELIANCE TCS            # Indian stocks are the default
stockwise TCS                           # no command needed: a one-screen overview
stockwise quote AAPL NVDA -m us         # US stocks
stockwise import kite tradebook.xlsx    # CSV or .xlsx; yearly tradebooks merge
stockwise portfolio xray                # your real exposure across stocks, funds and ETFs
stockwise tax in                        # LTCG/STCG per lot, with set-off rules
stockwise harvest                       # losses and gains to book before 31 March
stockwise tax advance --paid 2025-06-14:3000   # instalments and Sec. 234C interest
stockwise mf sip 122639 5000 -p 5y      # what a ₹5,000/month SIP became
stockwise backtest INFY -s macd         # a strategy after STT and brokerage
stockwise mcp                           # read-only tools for AI assistants
```

<p align="center">
  <img src="assets/history.svg" alt="stockwise history AAPL -m us -p 6mo" width="720">
</p>

## Where to next

- **[Cheatsheet](cheatsheet.md)** — the commands people use most, on one page.
- **[Commands](commands.md)** — every command and option, with the JSON each one prints.
- **[Troubleshooting](troubleshooting.md)** — Yahoo or NSE not answering, `Not enough price data`,
  import skips, where your data is.
- **[AI](ai.md)** — the MCP server for Claude, Cursor and other assistants, and `stockwise ask`.
- **[Markets](markets.md)** — India, US, global exchanges, crypto, FX, commodities.
- **[Simulator](simulator.md)** — practise on historical crashes and earn badges.
- **[Brokers](brokers.md)** — importing holdings, and optional live orders behind a preview.
- **[How StockWise compares](comparison.md)** with other tools.

StockWise is open source under the MIT licence:
[github.com/iamsaquib8/stockwise](https://github.com/iamsaquib8/stockwise).
It is for education and information, **not financial advice** — see the
[disclaimer](disclaimer.md).
