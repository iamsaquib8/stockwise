# What do you want to do?

The guides below are short walkthroughs. Each one starts from a goal, gives
commands you can copy, shows what the output looks like and ends with links to
the [commands reference](commands.md) for every option.

New to StockWise? Do the [quickstart](quickstart.md) first. It takes five
minutes.

## Pick your path

| I want to | Read | Main commands |
|---|---|---|
| See my real returns across brokers and mutual funds | [Import your trades and see true returns](guide-portfolio.md) | `import`, `portfolio`, `portfolio lots` |
| Estimate my capital-gains tax and fill my ITR | [File your Indian capital-gains tax](guide-tax.md) | `tax in`, `harvest`, `tax advance`, `tax export in` |
| Check a stock or fund quickly | [Check a stock or fund fast](guide-research.md) | `quote`, `deep`, `score`, `events`, `mf` |
| Put new money where I am underweight | [Rebalance to target weights](guide-rebalance.md) | `portfolio target`, `rebalance --inflow` |
| Ask Claude or Cursor about my portfolio | [Use StockWise from Claude or Cursor](guide-mcp.md) | `mcp` |
| Get a message when a price or RSI level is reached | [Set up alerts and the daemon](guide-alerts.md) | `alert`, `notify`, `daemon` |
| Test an idea on past data, or practise with paper money | [Backtest and practise on past markets](guide-practice.md) | `backtest`, `sim replay` |
| Use the numbers in a script or spreadsheet | [Script it with JSON, CSV and jq](guide-scripting.md) | `--json`, `--csv` |

## Where each guide fits

StockWise does four jobs. The guides follow them.

1. **One portfolio with true returns.** Start with
   [Import your trades](guide-portfolio.md), then
   [Rebalance](guide-rebalance.md).
2. **Indian capital-gains tax.** Read [File your Indian capital-gains
   tax](guide-tax.md). It needs the portfolio from job 1.
3. **Quick lookups.** [Check a stock or fund fast](guide-research.md) and
   [Set up alerts](guide-alerts.md).
4. **AI access with local data.** [Use StockWise from Claude or
   Cursor](guide-mcp.md). [Script it](guide-scripting.md) covers the same data
   for your own tools.

[Backtest and practise](guide-practice.md) sits beside these. It uses past
prices and paper money, never your portfolio.

## Not here?

- Every command and option is in the [commands reference](commands.md). The
  [cheatsheet](cheatsheet.md) lists the common ones on one page.
- Errors and empty results are covered in [Troubleshooting](troubleshooting.md).
- Live orders through a broker are in [Brokers](brokers.md). They move real
  money, so they have no guide here.

StockWise is for education and information. It is not financial advice. See the
[disclaimer](disclaimer.md).
