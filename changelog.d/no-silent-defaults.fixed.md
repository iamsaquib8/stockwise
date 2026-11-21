- `forecast` and `longterm` no longer assume 30% or 25% volatility when it can't be measured: bands and
  simulations show "missing". `forecast`'s "Expected Change" is now "Trend-line projection".
  Failed fundamentals sources are reported instead of dropped; `STOCKWISE_SEC_USER_AGENT` sets the
  contact SEC EDGAR asks for when it refuses requests (HTTP 403).
