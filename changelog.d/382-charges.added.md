- The `stockwise trade` order preview (and `--dry-run`) now lists estimated
  charges for NSE equity: brokerage and the DP charge on delivery sells at
  each broker's published rates, STT, the exchange transaction charge in
  force on the day, SEBI fee, stamp duty and GST, as delivery and as
  intraday side by side, plus an estimated slippage line from the bid-ask
  spread for MARKET and SL-M orders. STT, SEBI fee, stamp duty and GST use
  the backtest's own rates. `--dry-run --json` and `--dry-run --csv` print
  the preview and the charges as data. Part of
  [#382](https://github.com/iamsaquib8/stockwise/issues/382).
