- `chart`, `risk`, `verdict` and backtests keep working when Yahoo's price history fails: Indian stocks
  and NSE indices fall back to NSE (or BSE) history adjusted for splits and bonuses, then to bars cached
  in `cache.db`, with a warning that says how old they are.
