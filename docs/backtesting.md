# Backtesting

`stockwise backtest` replays a strategy over daily price history and reports
what it would have made **after trading costs**. It can test one stock, a
portfolio of stocks sharing one pot of capital, or run a walk-forward test
that tunes parameters on old data and scores them only on newer data.
Strategies are small TOML files; the six built-ins are files too.

> Backtests describe the past. They are not predictions and not financial
> advice — see [DISCLAIMER.md](../DISCLAIMER.md).

## Quick start

> **Guide:** [Backtest and practise on past markets](guide-practice.md) walks through a backtest and a replay.

```bash
stockwise backtest INFY -s macd                     # one stock, 2 years, Indian delivery costs
stockwise backtest TCS -s rsi -P oversold=25        # change a parameter
stockwise backtest AAPL -m us -s sma -p 5y          # US stock, US cost model
stockwise backtest portfolio TCS INFY HDFCBANK -s sma --rebalance monthly
stockwise backtest walkforward RELIANCE -s rsi --train 1y --test 3mo
stockwise backtest strategies                       # list built-in and your own strategies
stockwise backtest TCS -s file:my_rules.toml        # your own strategy file
```

Every form takes `--json`.

## How a backtest trades

- **Data.** Daily bars from Yahoo Finance for `-p` (default `2y`; `5y` for
  walk-forward). A bar missing any of open, high, low or close is dropped,
  never filled in. Prices are split-adjusted; dividends are not credited.
- **Signals at the close, fills at the next open.** Entry and exit rules are
  checked on each bar's close and filled at the **next bar's open** — the
  first price you could actually trade at. `--fill close` fills at the signal
  bar's close instead (how StockWise 0.3 and earlier filled); it peeks at a price that
  only exists once the bar is over, so it flatters results.
- **Long only, whole shares.** An entry buys as many whole shares as the cash
  allows after costs; an exit sells them all. Cash earns nothing. If the
  capital can't buy one share the entry is skipped and counted.
- **Entry and exit on the same bar** cancel: no new position is opened, and
  an open one is closed.
- **Stops and targets** (`stop_loss_pct`, `take_profit_pct` in a strategy
  file) are measured from the entry price and checked against each bar's low
  and high. A gap through the level fills at the open. If one bar touches
  both, the stop is assumed to have hit first.
- **The last bar.** A position still open at the end is sold at the last
  close, with costs, and listed as `still open`.

## Costs

Costs are on by default. `--costs auto` (the default) uses the crypto model
for coins (`-m crypto`, `BTC-USD`), the FX model for currency pairs (`-m fx`,
`USDINR=X`), India delivery for INR stocks, and the US model for everything
else. The Indian presets are refused for coins and currency pairs.

| `--costs` | What it charges |
|-----------|-----------------|
| `in-delivery` | Brokerage ₹20 or 0.1% per order, whichever is lower · STT 0.1% on buys and sells · exchange 0.00297% · SEBI ₹10/crore · GST 18% on brokerage + exchange + SEBI · stamp duty 0.015% on buys · DP charge ₹15.93 per sell |
| `in-intraday` | Brokerage ₹20 or 0.03%, lower · STT 0.025% on sells · exchange, SEBI and GST as above · stamp duty 0.003% on buys · no DP charge |
| `us` | Commission-free; slippage only |
| `crypto` | 0.1% exchange fee per order; 10 bps slippage |
| `fx` | No commission; 2 bps slippage for the spread |
| `none` | Nothing — the gross result |

The stock presets add **5 bps of slippage** per fill: buys fill 0.05% above
the price, sells 0.05% below. Override with `--slippage-bps`, and
brokerage with `--brokerage`: `20` (flat per order), `0.03%` (percent of the
order), or `20,0.03%` (whichever is lower). A Zerodha delivery account is
`--brokerage 0`. Rates are NSE equity rates as of 2025; check your broker's
contract note.

**Worked example** (delivery): buy 100 shares at ₹1,000 and sell at ₹1,100.

| | Buy (₹1,00,000) | Sell (₹1,10,000) |
|---|---:|---:|
| Brokerage | 20.00 | 20.00 |
| STT | 100.00 | 110.00 |
| Exchange | 2.97 | 3.27 |
| SEBI | 0.10 | 0.11 |
| GST | 4.15 | 4.21 |
| Stamp duty | 15.00 | — |
| DP | — | 15.93 |
| **Total** | **142.22** | **153.51** |

The ₹10,000 gross profit keeps ₹9,704.26 — before slippage. A unit test
checks these numbers.

Every report shows the **net** return (after costs), the **gross** return
(the same rules with `--costs none`), and the money paid in costs.

## What the numbers mean

| Metric | Meaning |
|--------|---------|
| Net / gross return | Change in portfolio value with / without costs |
| Buy & hold | The price change over the period (for a portfolio: weighted by the starting weights, no costs) |
| Alpha | Net return minus buy & hold |
| CAGR | Compound annual growth rate; shown for three months of data or more |
| Max drawdown | Largest fall from a peak, on the daily equity curve |
| Sharpe / Sortino | From daily returns of the equity curve, annualised with √252 (√365 for crypto, √260 for FX), risk-free rate 0 |
| Exposure | Average share of the portfolio invested rather than in cash |
| Win rate, avg win / loss | Per round trip, after costs |
| Profit factor | Money made on winners ÷ money lost on losers, after costs |

## Portfolio backtests

```bash
stockwise backtest portfolio TCS INFY HDFCBANK RELIANCE -s sma
stockwise backtest portfolio TCS INFY -s rsi --weights 0.7,0.3
stockwise backtest portfolio AAPL MSFT NVDA -m us -s macd --weights inverse-vol --rebalance monthly
```

Each symbol gets a sleeve of the capital — its weight — and trades the
strategy on its own; a flat sleeve holds cash. The report adds a per-symbol
table: trades, win rate, net P&L, **contribution** (sleeve P&L as a percent of
starting capital; the contributions add up to the total return), and how
often the symbol was held.

- `--weights equal` (default), explicit weights (`0.5,0.3,0.2`, normalised,
  one per symbol), or `inverse-vol`: weight ∝ 1 ÷ volatility of the last 63
  daily returns, computed at each rebalance from data before that day.
  `inverse-vol` needs `--rebalance`.
- `--rebalance none|monthly|quarterly`: on the first trading day of each
  month or quarter, at the open, overweight positions are trimmed, cash is
  moved between sleeves, and underweight positions are topped up, so every
  sleeve is back at its weight. Drifts under 1% are left alone. Trades pay
  costs as usual.
- Symbols are aligned by date; a symbol with no bar on a day keeps its last
  price. Every symbol must trade in the same currency and use the same cost
  model (stocks and coins can't share a portfolio backtest unless you pass
  one `--costs`).

## Walk-forward

A single backtest picks parameters with hindsight. Walk-forward avoids that:

```
history:  |----- train 1 -----|- test 1 -|
                    |----- train 2 -----|- test 2 -|
                              |----- train 3 -----|- test 3 -|
```

For each window, every parameter combination in the strategy's `[grid]` is
backtested on the train window; the one with the best Sharpe (among those
that traded) trades the following test window. Only test windows count. The
test windows are chained with compounding capital into one out-of-sample
equity curve, after costs.

Positions carry across windows: the chosen parameters trade from the start of
their train window, so a position they would already hold is open when the
test window starts, and a position open at a window's end is valued, not
sold — only the last window sells at the final close. A trade is listed in
the window it closes in. Switching parameters between windows is assumed to
cost nothing.

```bash
stockwise backtest walkforward RELIANCE -s rsi --train 1y --test 3mo -p 10y
```

- `--train` / `--test` take `90d`, `13w`, `6mo`, `1y`, counted in daily bars:
  for stocks a week is 5, a month 21 and a year 252 trading days; crypto
  trades every day (7, 30, 365); FX years are 260 weekdays. Train needs 20
  bars or more, test 5.
  Windows step forward by the test length; a short final test window is kept
  if it has at least half the test length.
- The report lists each test window with the chosen parameters, in-sample
  return and Sharpe, and out-of-sample return and Sharpe, then the stitched
  out-of-sample result and an **overfitting check**:
  - *Walk-forward efficiency*: out-of-sample annualised return ÷ the average
    in-sample annualised return.
  - **Verdict**: `inconclusive` with fewer than 2 windows or no
    out-of-sample trades; `no edge` if it loses money even in-sample;
    `likely overfit` if out-of-sample loses money, keeps under 30% of the
    in-sample return, or its Sharpe is more than 1.0 lower; `degraded` under
    60% or a Sharpe gap over 0.5; otherwise `robust`.
- A strategy without a `[grid]` has nothing to tune; walk-forward then checks
  how stable fixed parameters are across periods.

## Strategy files

`-s` takes:

1. A built-in: `rsi`, `macd`, `sma` (`sma_crossover`), `bb` (`bollinger`),
   `vwap`, `mr` (`mean_reversion`).
2. The name of a file in your strategies folder: `-s trend_pullback` loads
   `trend_pullback.toml` from
   - `strategies/` next to your config file — `stockwise config path` shows
     where that is (`~/Library/Application Support/stockwise/` on macOS,
     `~/.config/stockwise/` on Linux, `%APPDATA%\stockwise\` on Windows), or
   - `$STOCKWISE_HOME/strategies/` when `STOCKWISE_HOME` is set.

   `stockwise backtest strategies` prints the folder it uses.

   Built-in names win over files of the same name. A `NAME.wasm` plugin in
   the folder is found too (after `NAME.toml`).
3. A path: `-s file:path/to/rules.toml` (or any path ending in `.toml`).
4. A WASM plugin: `-s wasm:path/to/strategy.wasm` — see
   [WASM plugins](#wasm-plugins). Needs a build with `--features wasm-plugins`.

The built-ins live in [`examples/strategies/`](../examples/strategies/) next to
two more examples (`trend_pullback`, `breakout`) — copy one to start.
`stockwise backtest strategies` lists everything and checks your files;
`stockwise backtest strategies FILE...` checks specific files. Both exit 1 if
a file has errors.

### Format

```toml
name = "trend_pullback"                 # optional; defaults to the file name
description = "Buy RSI dips above the 200-day average"
entry = ["close > sma(200)", "rsi(14) < $dip"]   # all must hold
exit = "rsi(14) > 70 or close < sma(200)"
stop_loss_pct = 8                       # optional, from the entry price
take_profit_pct = 15                    # optional

[params]                                # numbers, used as $name
dip = 35

[grid]                                  # optional: values walk-forward tries
dip = [30, 35, 40]
```

| Key | |
|-----|---|
| `entry` | A condition, or a list of conditions that must all hold. Required |
| `exit` | Same. Optional when `stop_loss_pct` or `take_profit_pct` is set |
| `stop_loss_pct` | Exit when the price falls this far below the entry (0–100) |
| `take_profit_pct` | Exit when the price rises this far above the entry |
| `[params]` | Default parameter values; override with `-P name=value` (repeatable). A `-P` value also fixes that parameter for walk-forward |
| `[grid]` | For each parameter, the values walk-forward tries (at most 500 combinations) |

Unknown keys are errors, so a typo like `stop_los_pct` is caught.

### Conditions

A condition compares two values:

| Operator | True on a bar when |
|----------|--------------------|
| `<` `<=` `>` `>=` | the comparison holds |
| `a crosses_above b` (or `crosses above`) | `a > b` on this bar and `a <= b` on the previous one |
| `a crosses_below b` (or `crosses below`) | `a < b` on this bar and `a >= b` on the previous one |

Combine conditions with `and`, `or`, `not` (or `&&`, `||`) and parentheses.
Values can be numbers, `$params`, the series below, and `+ - * /` between
them: `close < sma(20) * (1 - $band_pct / 100)`. Keywords and indicator names
are case-insensitive; parameter names are not.

| Series | |
|--------|---|
| `close` `open` `high` `low` `volume` | The bar's prices and volume |
| `sma(n)` `ema(n)` | Moving averages of the close |
| `rsi(n)` | Wilder RSI of the close |
| `atr(n)` | Average true range |
| `macd` `macd_signal` `macd_hist` | MACD(12, 26, 9) line, signal line, histogram; or `macd_hist(fast, slow, signal)` |
| `bb_upper(n)` `bb_middle(n)` `bb_lower(n)` | Bollinger Bands, 2 standard deviations; `bb_upper(n, k)` for k |
| `vwap` | Volume-weighted average price from the first bar |
| `highest(n)` `lowest(n)` | Highest high / lowest low of the previous `n` bars (not including this one) |

These are the indicators of `src/technical.rs`; a test checks that every value
matches. Periods are whole numbers from 1 to 1000 and may be `$params`.

While an indicator is warming up it has no value, and a condition that uses
it is neither true nor false — so nothing fires, even under `not`. Division
by zero behaves the same way.

The file is checked before anything runs, with messages that name the file
and the condition:

```
Error: rules.toml: entry "rsx(14) < 30": unknown indicator `rsx`. Available: close, open, ...
Error: rules.toml: `$dip` is used in a condition but not defined under [params]
Error: strategy `rsi` has no parameter `oversld` (it has: overbought, oversold, period)
```

## Strategies in the intraday bot

```bash
stockwise intraday 50000 1.5 -s trend_pullback
```

`intraday -s` adds a strategy file as one more signal next to the bot's nine:
a **Long** vote (strength 70) when its entry rule holds on the latest daily
bar, a **Short** vote (strength 60) when its exit rule holds. The rule gets
its own daily history (one year, or more for slow indicators), so the
built-in signals are unchanged. The vote shows as `Custom rule` in the plan's
strategy list.

Not yet: `daemon intraday` and `sim start` don't take `-s`.

## WASM plugins

**Experimental.** When a strategy needs state or logic that TOML conditions
can't express — a custom indicator, a regime filter, a model — write it in
any language that compiles to WebAssembly and load it with `-s wasm:PATH`:

```bash
cargo install stockwise --features wasm-plugins

cd examples/wasm-strategy-rsi
cargo build --target wasm32-unknown-unknown --release   # rustup target add wasm32-unknown-unknown
cd -

P=examples/wasm-strategy-rsi/target/wasm32-unknown-unknown/release/wasm_strategy_rsi.wasm
stockwise backtest RELIANCE -s wasm:$P -P oversold=25
stockwise backtest portfolio TCS INFY HDFCBANK -s wasm:$P
stockwise backtest walkforward TCS -s wasm:$P
stockwise intraday 50000 1.5 -s wasm:$P
stockwise backtest strategies $P        # check a plugin: name, ABI, parameters
```

A plugin works everywhere a TOML strategy does: single, portfolio and
walk-forward backtests, `-P` overrides, `[grid]`-style tuning, and the
intraday bot's extra vote. Copy it into your strategies folder as
`NAME.wasm` to run it as `-s NAME`; `backtest strategies` lists it with its
declared name and ABI version. `-s wasm:FILE.wat` also loads WebAssembly
text, handy for tiny experiments.

[`examples/wasm-strategy-rsi`](../examples/wasm-strategy-rsi/) is a complete
plugin in Rust with no dependencies: RSI mean reversion with exactly the
built-in `rsi` strategy's signals (a test checks every grid combination).
[`src/backtest/fixtures/level.wat`](../src/backtest/fixtures/level.wat) is the
smallest complete one, in WebAssembly text.

### Sandbox

- **No imports.** A plugin gets no host functions: no WASI, files, network,
  clock or randomness. A module that imports anything is refused at load
  (build for `wasm32-unknown-unknown`, not `wasm32-wasip1`).
- **Fuel.** Each call runs on a budget of about one unit per instruction:
  10 million for a `stockwise_on_bar` call, 50 million for instantiation
  and each setup call. An endless loop stops with an error at the first bar.
- **Memory.** 64 MB of linear memory per instance, one memory, plugin files
  up to 16 MB, and wasmi's strict limits on module structure (functions,
  globals, tables, data segments).
- **Errors, not crashes.** A trap (a Rust panic, an out-of-bounds access,
  division by zero, stack overflow), running out of fuel or memory, or a
  return value outside the ABI ends the command with exit code 1 and a
  message naming the plugin, the call and the bar:

  ```
  Error: loop.wasm: `stockwise_on_bar` on bar 1 of 496 (2024-10-23): ran past its budget of 10000000 instructions for one call — an endless loop?
  Error: src/backtest/fixtures/level.wat: the plugin rejected {"level":0.0} (`stockwise_init` returned 1): level must be a whole number above 0
  ```
- **No look-ahead.** The plugin sees bars one at a time, oldest first, so a
  signal can only depend on the bars before it — the same guarantee as TOML
  rules. Signals take effect as usual: at the next bar's open by default.

### ABI (version 1)

A plugin is a WebAssembly module with these exports. Integers are `i32` /
`i64`; strings cross as UTF-8 in the plugin's exported `memory`, returned as
one `i64` packing `(pointer << 32) | length`.

| Export | Signature | |
|--------|-----------|---|
| `stockwise_abi_version` | `() -> i32` | Required. Returns `1` |
| `stockwise_on_bar` | `(time: i64, open: f64, high: f64, low: f64, close: f64, volume: f64) -> i32` | Required. One call per daily bar, oldest first; returns the signal at that bar's close |
| `memory` | memory | Required by the exports below |
| `stockwise_meta` | `() -> i64` | Optional. Packed pointer/length of the metadata JSON |
| `stockwise_alloc` | `(len: i32) -> i32` | With `stockwise_init`. A buffer of `len` bytes for the parameters |
| `stockwise_init` | `(ptr: i32, len: i32) -> i32` | Optional; required when `params` are declared. Receives the parameters; returns `0`, or anything else to refuse them |
| `stockwise_error` | `() -> i64` | Optional. Packed pointer/length of a message explaining the last refusal or error |

**Signals.** `stockwise_on_bar` returns `0` (nothing), `1` (entry), `2`
(exit) or `3` (both: an exit while holding, nothing when flat — as when a TOML
strategy's entry and exit rules both hold). A negative value is an error
reported by the plugin (with `stockwise_error`'s message); anything else is
refused as bad output. `time` is the bar's start in Unix seconds (UTC);
`volume` may be 0.

**Lifecycle.** At load, StockWise instantiates the module once to check the
exports and their signatures, call `stockwise_abi_version` and read
`stockwise_meta`. Then every price series — each symbol, each parameter set
in walk-forward, each stock the intraday bot scans — gets a **fresh
instance**: `stockwise_alloc(len)`, the parameters written there,
`stockwise_init(ptr, len)`, then `stockwise_on_bar` for each bar. Keep state
in globals; it never leaks between series. Parameter defaults and every
`grid` combination are tried through `stockwise_init` at load, so a plugin
can refuse values up front.

**Metadata** (`stockwise_meta`), every key optional, unknown keys refused:

```json
{
  "name": "rsi-wasm",
  "description": "RSI(14) mean reversion as a WASM plugin",
  "params": { "period": 14, "oversold": 30, "overbought": 70 },
  "grid": { "oversold": [25, 30, 35], "overbought": [65, 70, 75] },
  "lookback": 15,
  "stop_loss_pct": 8,
  "take_profit_pct": 15
}
```

`name` defaults to the file name. `params` and `grid` follow the TOML rules
(names of letters, digits and `_`; at most 500 grid combinations); `-P` sets
a parameter and fixes it for walk-forward. `lookback` (default 0, at most
10,000) is how many bars the plugin needs before its first signal: backtests
refuse shorter histories and the intraday bot fetches enough.
`stop_loss_pct` and `take_profit_pct` work as in TOML.

**Parameters** reach `stockwise_init` as compact JSON: one flat object of
every parameter, names sorted, numbers as JSON numbers —
`{"overbought":70.0,"oversold":25.0,"period":14.0}`. Keys never contain
`,` `:` or quotes, so a small hand-written reader is enough (the example
does this).

### Runtime

Plugins run in [wasmi](https://github.com/wasmi-labs/wasmi) 2.x, a pure-Rust
WebAssembly interpreter. It was chosen over wasmtime because:

- **Size and build time.** With the feature, the release binary grows by
  2.4 MB (12.9 → 15.3 MB on aarch64 macOS): 1.5 MB for wasmi and 0.9 MB for
  the WAT text parser. Its crates add about 35 s of CPU time to a release
  build, mostly in parallel with the rest. wasmtime with Cranelift is several
  times larger, slower to compile, and pulls in a much bigger dependency
  tree.
- **MSRV.** wasmi 2.0 builds on Rust 1.86, well inside StockWise's 1.97.
  wasmtime requires the newest compilers (wasmtime 50 needs 1.97, and each
  monthly release moves that up), which would tie StockWise's MSRV to
  wasmtime's release train.
- **Limits.** wasmi meters fuel deterministically per instruction and
  enforces memory, table and module-structure limits — what a plugin sandbox
  needs. wasmtime's extra features (JIT speed, epoch interruption, WASI) are
  unneeded: a plugin does a few hundred instructions per bar, and has no
  host access.
- **No unsafe host surface.** No imports are linked, the module is validated
  before it runs, and an interpreter cannot jump to plugin-generated machine
  code.

The feature is off by default, so default builds and binaries are unchanged.

## JSON output

`--json` prints one object. Percentages end in `_percent`; money is in the
stock's currency.

- **`backtest SYMBOL`**: `symbol, currency, strategy, period, interval, bars,
  start, end, fill, capital`, the metrics (`start_value, end_value,
  total_return_percent, cagr_percent, max_drawdown_percent, sharpe, sortino,
  exposure_percent, trade_count, win_rate_percent, avg_win_percent,
  avg_loss_percent, profit_factor, costs_paid`), `gross_return_percent,
  buy_hold_return_percent, alpha_percent, skipped_entries`, `costs`
  (`brokerage, stt, exchange, sebi, gst, stamp, dp, slippage, total`),
  `cost_model`, `rules` (the strategy as loaded, with `-P` applied),
  `trades: [trade]` and `equity: [{date, value}]`.
  A trade is `{symbol, entry_date, exit_date, qty, entry_price, exit_price,
  gross_pnl, costs, net_pnl, pnl_percent, bars_held, exit_reason}` with
  `exit_reason` one of `signal, stop-loss, take-profit, rebalance, end-of-data`.
- **`backtest portfolio`**: the same, with `symbols, weights, rebalance,
  rebalances` and `per_symbol: [{symbol, weight, pnl, contribution_percent,
  trades, win_rate_percent, costs, exposure_percent}]`.
- **`backtest walkforward`**: `symbol, currency, strategy, period, interval, fill,
  capital, cost_model, rules, train_days, test_days, combos`,
  `folds: [{params, in_sample, out_of_sample}]` (each window `{start, end,
  bars, return_percent, annualised_percent, sharpe, max_drawdown_percent,
  trade_count}`), `out_of_sample` (the metrics), `out_of_sample_gross_return_percent,
  out_of_sample_annualised_percent, in_sample_annualised_percent,
  in_sample_sharpe, efficiency, verdict, verdict_reason, trades, equity`.
- **`backtest strategies`**: `{builtin: [{aliases, strategy}], directory,
  user: [{path, ok, strategy | error}], skipped_plugins: [path]}`.
  `skipped_plugins` lists `.wasm` files a build without `wasm-plugins` did
  not load.

For a WASM plugin, `rules` (and `strategy` above) has empty `entry` and
`exit`, the plugin's `params` and `grid`, and `plugin: {abi_version,
lookback}`.

```bash
for s in rsi macd sma bb vwap mr; do
  stockwise backtest INFY -s "$s" --json | jq -r '"\(.strategy)\t\(.total_return_percent)"'
done
```

## Limits

- Long only; no short selling, leverage or F&O.
- Daily bars only. Intraday costs (`--costs in-intraday`) can be applied, but
  positions are still held from one daily bar to another.
- Dividends are not credited; symbols that were delisted are not in Yahoo's
  data (survivorship bias).
- WASM plugins are **experimental** and opt-in (`--features wasm-plugins`);
  the ABI may change in a later version, which bumps `abi_version`.
