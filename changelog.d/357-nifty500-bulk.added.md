- `--universe nifty500|midcap150|smallcap250`: one NSE request prices the whole
  index, so `movers` and `heatmap` work when Yahoo Finance is unreachable and
  every index scan feeds `market_snapshots`
  ([#357](https://github.com/iamsaquib8/stockwise/issues/357)).
