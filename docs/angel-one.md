# Angel One Live Trading

StockWise integrates with Angel One SmartAPI (free) for live order placement.

## Setup

1. Create an account at [angelone.in](https://www.angelone.in/)
2. Register an app at [smartapi.angelone.in](https://smartapi.angelone.in/)
3. Enable TOTP on your account and note the base32 secret

```bash
stockwise trade config
```

Edit the generated file at `~/Library/Application Support/stockwise/angel_config.json`:

```json
{
  "api_key": "YOUR_API_KEY",
  "client_id": "YOUR_CLIENT_ID",
  "password": "YOUR_PASSWORD",
  "totp_secret": "YOUR_TOTP_BASE32_SECRET"
}
```

## Commands

```bash
stockwise trade holdings              # Live holdings (auto-syncs locally)
stockwise trade positions             # Open intraday positions
stockwise trade buy RELIANCE 10       # Market buy
stockwise trade sell TCS 5            # Market sell
stockwise trade limit INFY 20 1300    # Limit buy
stockwise trade orders                # Today's order book
```

## Security

- Credentials stored locally only, never sent except to Angel One's API
- TOTP generated locally via built-in HMAC-SHA1 (no external dependencies)
- No data leaves your machine except direct Angel One API calls

## Broker Import

Import holdings from other brokers without manual entry:

```bash
stockwise import kite ~/Downloads/holdings.csv       # Zerodha Kite
stockwise import indmoney ~/Downloads/portfolio.csv  # IndMoney
stockwise import csv ~/Downloads/stocks.csv          # Any CSV
```

Auto-detects column names: Symbol/Instrument, Quantity/Qty, Avg Cost/Average Price.
