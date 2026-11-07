- `levels`, `deep`, `technical` and `intraday`: a symbol's own market now decides whether today's
  session is still running and how many bars make a year, instead of the market you ran the command
  with (India by default). `levels BTC-USD` or `levels ^GSPC` on a weekday evening no longer builds
  pivots from today's half-finished bar and calls them the prior session.
