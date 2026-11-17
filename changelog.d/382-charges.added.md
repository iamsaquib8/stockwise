- The `stockwise trade` order preview (and `--dry-run`) now lists estimated
  charges for NSE equity: brokerage at each broker's published rates, STT,
  exchange transaction charge, SEBI fee, stamp duty, GST and the DP charge on
  delivery sells, as delivery and as intraday side by side, plus an estimated
  slippage line from the bid-ask spread for MARKET and SL-M orders. The
  statutory rates are the backtest's own. Part of
  [#382](https://github.com/iamsaquib8/stockwise/issues/382).
