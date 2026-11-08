- `stockwise portfolio sell SYMBOL QTY [PRICE] --preview` (alias `--dry-run`)
  shows the FIFO lots a sale would take, the short- and long-term capital-gains
  tax with the unused Sec. 112A exemption, grandfathered cost and 4% cess, a
  countdown for lots within 45 days of turning long-term (with the tax saved by
  waiting) and a mutual-fund exit-load check, without changing `portfolio.json`.
  `sellpreview::ltcg_countdown` is a pure function the background worker can
  call for post-close notifications. New MCP tool `portfolio_sell_preview`
  ([#375](https://github.com/iamsaquib8/stockwise/issues/375)).
