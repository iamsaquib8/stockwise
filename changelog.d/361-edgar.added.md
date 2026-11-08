- `stockwise results AAPL -m us` reads SEC EDGAR (keyless `companyfacts` XBRL
  from 10-K and 10-Q filings) for 12 or more quarters and the fiscal years
  behind them, instead of Yahoo's last four: revenue, operating income, net
  income, diluted EPS, operating cash flow and capex. Q4 is derived as the
  fiscal year minus Q1 to Q3, cash-flow quarters from year-to-date
  differences, restated facts take the latest filing, and September or
  52/53-week year ends work. Yahoo fills the fields EDGAR lacks and the
  source is listed; periods are stored in `cache.db` tagged `edgar`. Requests
  carry a generic `stockwise` User-Agent (no email) and stay under 5 a second.
