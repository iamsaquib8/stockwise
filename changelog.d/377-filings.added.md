- The background worker's post-close sync now also notifies about new
  non-routine NSE exchange filings of held `.NS` stocks (date, title and
  category, plus `stockwise events SYMBOL`), once per filing; the `eod` task
  result reports a `filings` count
  ([#377](https://github.com/iamsaquib8/stockwise/issues/377)).
