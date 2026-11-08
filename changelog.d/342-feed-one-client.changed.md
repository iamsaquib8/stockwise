- `daemon intraday` keeps one market-data client for the life of the loop
  instead of rebuilding it after every third failed tick; the client already
  renews its own session, so a failure now only changes the retry delay
  ([#342](https://github.com/iamsaquib8/stockwise/issues/342)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
