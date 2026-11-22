- `sim`, `bot` and `daemon intraday`: paper P&L is now after brokerage, STT,
  exchange and stamp charges and slippage (set with `STOCKWISE_SIM_COSTS`,
  `STOCKWISE_SIM_BROKERAGE`, `STOCKWISE_SIM_SLIPPAGE_BPS`). Exit levels sit at
  least 1 × ATR away, each trade risks a fixed 0.5% until 30 closed paper
  trades allow measured half-Kelly, plans rank by score, oversold readings
  share one vote, MACD reads three months of bars, VWAP and volume pace come
  from today's 5-minute bars, and Sharpe counts every trading day.
