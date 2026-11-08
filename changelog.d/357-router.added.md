- Provider router: `FINNHUB_API_KEY` (quotes, profile, analyst trend, earnings
  surprises) and `FMP_API_KEY` (quotes, quarterly statements, screener) back
  Yahoo up per capability, respecting each free tier's window and merging
  fields with `meta.sources`; `results` and US scans keep working when Yahoo
  fails, `doctor` and `config show` list the keys, and the BSE provider reads
  announcements, results filings and group members
  ([#357](https://github.com/iamsaquib8/stockwise/issues/357)).
