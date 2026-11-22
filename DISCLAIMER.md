# Disclaimer

**StockWise gives rule-based educational signals. It is not personalised
financial, investment, trading, legal, or tax advice.**

- **Actions are rule-based signals, not advice.** StockWise shows actions
  (BUY, WATCH, AVOID for a stock you don't hold; ADD, HOLD, TRIM, EXIT for one
  you do) computed by published, deterministic rules (see
  [docs/methodology.md](docs/methodology.md), "Actions"). Each comes with its
  reasons, an invalidation price and a review date. When the data is too thin
  it says NO CALL. The rules do not know your full finances, goals or tax
  position, so the signals are not personalised advice.
- **StockWise is not a registered adviser.** It is not a SEBI-registered
  investment adviser or research analyst, and it is not a registered
  investment adviser, broker-dealer or financial adviser in any other country.
- **You decide, and you are responsible.** Check the reasons and the data
  before you act. Every decision, and its result, is yours.
- **You can lose money.** Markets are risky. Prices can fall below an
  invalidation level before you act on it, and you can lose part or all of
  your capital, or more than you invest with leverage or derivatives. Quotes,
  scores, technical signals, backtests, picks, AI answers, forecasts and
  daemon decisions are not promises of any result.
- **Data can be wrong or stale.** All data comes from third-party providers
  (e.g. Yahoo Finance, NSE, BSE) on an "as available" basis. It may be delayed,
  inaccurate, incomplete, or wrong, and an action computed from it is only as
  good as that data. Verify it independently before you rely on it.
- **AI only explains.** AI answers (`ask`, `--ai`, MCP assistants) describe the
  action the rules computed. If an AI states an action or a price level that
  StockWise did not compute, ignore it.
- **Live trading places real orders with real money.** The `trade buy`, `sell`
  and `limit` commands send orders to your connected broker (e.g. Angel One)
  after showing a preview and asking you to confirm — or immediately when you
  pass `--yes`. The `daemon` workers paper-trade only and do not send orders.
  You are solely responsible for every order placed, for the configuration of
  risk controls, and for any financial loss. Test with paper trading (`sim`)
  and `trade ... --dry-run` first.
- Backtests and simulations describe the past or a hypothetical. **Past
  performance does not guarantee future results.** Simulated results have
  inherent limitations and do not represent actual trading.
- The software is provided "AS IS", without warranty of any kind, under the
  terms of the [MIT License](LICENSE). The authors accept no liability for any
  loss or damage arising from its use.
- StockWise is an independent open-source project. It is **not affiliated with,
  endorsed by, or sponsored by** Yahoo, Angel One, Zerodha, the NSE, the BSE, or
  any exchange, broker, or data provider.

For advice that fits your situation, consult a SEBI-registered investment
adviser (or a licensed adviser where you live). Use of this software
constitutes acceptance of this disclaimer. If you do not agree, do not use it.
