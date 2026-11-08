- `serve` stops exporting a daemon's session gauges from `/metrics` once its
  saved date is no longer the current session, so yesterday's open positions no
  longer read as today's
  ([#342](https://github.com/iamsaquib8/stockwise/issues/342)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
