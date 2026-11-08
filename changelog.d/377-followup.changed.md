- Redis is now opt-in: without `STOCKWISE_REDIS_URL` or `redis_url`, StockWise
  no longer probes `127.0.0.1:6379` and uses the SQLite `cache.db` instead.
  Chart fetches also keep their complete OHLCV bars in `cache.db`, and
  `worker start|stop` works on Windows
  ([#377](https://github.com/iamsaquib8/stockwise/issues/377)).
