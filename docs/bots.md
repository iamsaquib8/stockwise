# Trading Bots

## Intraday Bot

9-strategy multi-signal engine for day trading.

```bash
stockwise bot 25000 2       # ₹25K capital, 2% daily target
stockwise -m us bot 5000 1  # US market, $5K, 1% target
```

### Strategies

| # | Strategy | Signal | Trigger |
|---|----------|--------|---------|
| 1 | RSI Reversal | Long/Short | RSI <30 (oversold) or >70 (overbought) |
| 2 | VWAP Reclaim | Long | Price crosses above VWAP from below |
| 3 | BB Squeeze | Long | Tight Bollinger bands + price above middle |
| 4 | BB Bounce | Long | Price touching lower Bollinger Band |
| 5 | MACD Cross | Long/Short | Histogram sign change (fresh crossover) |
| 6 | Volume Breakout | Long | 2x+ avg volume with positive price action |
| 7 | Support Bounce | Long | Price at pivot S1 level |
| 8 | OBV Divergence | Long | Volume accumulating while price flat/down |
| 9 | Mean Reversion | Long | Price >3% below 20-SMA |

### Scoring

- Each strategy generates a strength score (0-100)
- Consensus bonus: +8 per additional agreeing strategy
- Volume multiplier: 1.15x if volume >1.5x average
- Regime multiplier: 1.2x if trade aligns with strong trend, 0.6x if counter-trend
- Confidence: HIGH (score >=75, 3+ strategies), MEDIUM (>=60, 2+), LOW (others)

### Position Sizing (Kelly Criterion)

- Half-Kelly fraction for safety (max 25% of capital)
- 1% max risk per trade
- Never more than 30% of capital in one position
- At most 5 plans, and **together they never need more than your capital**:
  walking down the ranking, a plan that doesn't fit in what's left is shrunk
  to what does (or dropped)
- Exit level for each paper position: uses support levels when available, else ATR-based

### Output

- Sector heat map (which sectors are moving)
- Market regime detection (Strong Uptrend → Strong Downtrend)
- Top 15 signals ranked by score
- The strategies behind the five strongest signals, with each one's strength and reason (`intraday` shows no position sizes or price levels; the paper-trading `daemon` and `sim` still size simulated trades and show them as long or short with level 1, level 2 and an exit level)

---

## Long-Term Scores

6-pillar scoring of the market's stock universe with a Monte Carlo illustration. It shows data and how each score was built: no buy, sell or hold calls, no entries, stops or targets, and no allocation advice.

```bash
stockwise longterm 15000         # ₹15K/month SIP budget
stockwise -m us longterm 1000    # US market, $1K/month
```

### 6 Scoring Pillars

| Pillar | Weight | Key Metrics |
|--------|--------|-------------|
| Valuation | 18% | P/E, PEG ratio, P/B, forward P/E compression, EV/EBITDA, earnings yield |
| Growth | 22% | Revenue growth, quarterly earnings growth, EPS trajectory |
| Quality | 25% | Profit margins, ROE, debt/equity, current ratio |
| Momentum | 10% | Price against the 50- and 200-day averages, analyst mean rating, analyst mean price estimate against the price |
| Dividend | 10% | Yield level, DRIP compounding |
| Safety | 15% | Beta, market cap, historical volatility, debt levels |

### Moat Analysis

Stocks get Wide/Narrow/No Moat signals from these rules:
- Profit margins >25% (pricing power)
- ROE >25% (capital efficiency)
- Revenue growth >25% (market dominance)
- Market cap >$1T (scale advantage)

### Monte Carlo Simulation

- 1000 simulated 5-year paths using historical volatility
- Reports: P10 (worst case), Median, P90 (best case) returns
- Uses deterministic PRNG for reproducibility

### Output

- Top 12 names ranked by composite score, with each pillar's score (a dash where the pillar had no data; `n/a` when under half the weight had data)
- Detailed top 3: score band, data coverage, risk level (Lower / Moderate / Higher), PEG, earnings yield, and the facts that added or took away points
- 5Y and 10Y growth at an *assumed* annual return set from the score (not a forecast), with dividends reinvested
- SIP arithmetic (the monthly amount that would reach ₹10L in 10 years at that rate, and what your budget compounds to)
- A simulated 5-year range: 10th percentile, median and 90th percentile

---

## Paper Trading Simulator

Test the intraday bot without risking real money.

```bash
stockwise sim start 25000 2    # Morning: lock in trades
stockwise sim status           # During day: live P&L
stockwise sim settle           # After the close: replay the day, record results
stockwise sim history          # Performance over time
```

`sim start` only runs on a trading day before the close (the clock is IST, or
ET with `-m us`, and knows exchange holidays), and records when each trade
was entered.

### How `sim settle` decides

Each open session is settled from **its own day's 5-minute bars**, not from
whatever the price is when you run the command:

1. Bars before the entry time are ignored.
2. The bars are walked with the same position manager as the daemon: book
   half at level 1 and move the exit level to the opening price, close the
   rest at level 2, trail the exit level after level 1, tighten it in the
   wind-down. (Levels are fixed when the paper position opens; they are
   simulation mechanics, not advice.)
3. Whichever of the exit level or level 2 the price reached first decides the
   trade. Inside a single bar the order of the high and low is unknown, so a
   bar that touched both counts as **the exit level** (conservative). Levels
   fill at their price; a bar that opens beyond one fills at the open.
4. Otherwise the trade exits at the open of the 15:15 IST bar (15:45 ET).

Yahoo keeps 5-minute bars for about 60 days. Older sessions fall back to
hourly bars, then to the daily bar (same rules, exit at the close). Today's
session is refused until the market closes; `sim settle --now` settles it at
the latest bar. A session whose bars can't be fetched stays open and the
command exits non-zero if nothing was settled.

### What It Tracks

- Per trade: opening and closing price, whether level 2 or the exit level was reached, P&L, strategies used
- Per day: total P&L, win/loss count, how often level 2 was reached
- Cumulative: equity curve, Sharpe ratio, win streaks, day/trade win rates

### Confidence Rating (after 5+ days)

| Rating | Criteria |
|--------|----------|
| HIGH | >65% day win rate + >55% trade win rate + profitable |
| MODERATE | >50% day win rate + profitable |
| LOW | Below moderate threshold |

### Practice mode

To practise your own decisions rather than the bot's, replay a past market
day by day with the future hidden — the COVID crash, 2008, the dot-com bust —
take on goal-based challenges, and get a risk-adjusted score, badges and a
shareable card: `stockwise sim replay start covid-crash-2020`. See
[simulator.md](simulator.md).

---

## Daemon Mode

> **Guide:** [Set up alerts and the daemon](guide-alerts.md#5-run-the-paper-trading-daemon-optional).

Persistent background workers that run the bots continuously.

### Intraday Daemon

```bash
stockwise daemon intraday 25000         # NSE, ₹25K paper capital
stockwise -m us daemon intraday 5000    # NYSE hours (ET), $5K
stockwise daemon intraday 25000 --once  # one session, then exit
```

The daemon keeps running: after it settles a session it waits for the next
trading day (skipping weekends and exchange holidays) and runs that one, so
one process covers every day until you stop it with Ctrl+C or SIGTERM.
Start it any time: before or during a session it picks that session up (a
late start enters no new positions); after the close or on a weekend it waits
for the next pre-market. Add `--once` to run a single session and exit with
status 0 (for example from cron or a systemd timer).

Lifecycle (NSE; NYSE uses the same offsets from 09:30–16:00 ET):
```
9:00 AM   Pre-market scan (sectors, gaps)
9:15 AM   Enter up to 5 positions (9 strategies, Kelly sizing, ≤ capital in total)
9:30–3:15 Monitor every 60 seconds:
          - Level 1 reached → book 50%, move the exit level to the opening price
          - Level 2 reached → close fully
          - Exit level reached → close, record the loss
          - Trailing exit level updates (1.5% trail)
          - Wind-down (2 PM+): tighten the exit level to 0.5%
3:15 PM   Square off all remaining positions
3:20 PM   Settle → save to sim_history.json → AI EOD report → wait for the
          next session (with --once: exit)
```

The clock is the exchange's, not your machine's: IST (+05:30) for NSE, US
Eastern with daylight saving for NYSE. Weekends and exchange holidays are
closed all day; the daemon prints the reason and the next session, then
sleeps until 15 minutes before it.

Risk controls:
- 1% max risk per trade, 30% max per position, total allocation ≤ capital
- 3% daily loss kill switch (closes everything; counts every realised exit,
  including level 2 exits)
- Max 5 concurrent positions
- Entries only in the first 15 minutes; no new entries after that
- Forced square-off 15 minutes before the close

Resilience:
- One Yahoo session is reused across ticks. A failed tick is logged and
  retried after 15 s, 30 s, 60 s … (capped at 5 minutes); after three
  failures in a row the session is rebuilt. Network blips never stop it.
- The day's state (positions, risk manager, entries) is saved to
  `daemon_state.json` (`daemon_state_us.json` with `-m us`) in the data
  directory after every tick. Restarting the daemon the same day resumes it —
  the capital from the saved state wins.
- **Ctrl+C** (or SIGTERM, e.g. from systemd) squares off open positions at
  their last price, records the day in the simulator history, saves, and
  exits 0. A second Ctrl+C exits immediately; the saved state is resumed next
  time.
- If the daemon missed the square-off (crash, laptop asleep), it hands that
  day's entries to the simulator as an open session instead of closing them
  at stale prices: run `stockwise sim settle` to settle them from the day's
  5-minute bars.
- One NSE and one NYSE daemon can run side by side; don't start two for the
  same market — they would share one state file.

The daemon is **paper-only**: it never calls a broker order API. Results are
saved to simulation history — check with `stockwise sim history`. With
[notifications](#notifications) configured, it also sends entries, exits,
the kill switch and the end-of-day summary.

### Market calendar

Holidays come from a table in `src/clock.rs`: the NSE equity-segment list
(2026 official; 2027 provisional — only fixed-date holidays until NSE
publishes its circular in December) and the NYSE calendar for 2026–2028,
including 1:00 pm early closes. When the current year's list is missing or
provisional, the daemon prints a warning. Muhurat trading sessions are not
traded.

### Long-Term Daemon

```bash
stockwise daemon longterm
```

Runs 5 steps:
1. Score the market's universe (NIFTY 500 or S&P 500) on 6 pillars + Monte Carlo and print the three highest scores
2. Check portfolio value, snapshot wealth tracker
3. Check all price alerts
4. List holdings priced below their average cost
5. Generate AI research notes via Ollama

Run daily after market close, or weekly for deep analysis.

### Deep Dive

One command for everything about a single stock:

```bash
stockwise deep RELIANCE
```

Shows: company info, price action, valuation (P/E, PEG, P/B, EV/EBITDA), profitability, financial health, dividends, analyst ratings, 1Y chart, all technicals (RSI + gauge, SMA, MACD, Bollinger, ATR, VWAP), support/resistance, Fibonacci, risk (vol, Sharpe, drawdown, VaR), OBV trend, open gaps, insights, the 6-pillar score, and (with `--ai`) AI analysis.

---

## Price and Technical Alerts

> **Guide:** [Set up alerts and the daemon](guide-alerts.md) walks through alerts, notifications and cron.

```bash
stockwise alert add TCS above 4200
stockwise alert add AAPL -m us below 180
stockwise alert add INFY rsi-below 30         # RSI(14) at or below 30
stockwise alert add RELIANCE cross-above-sma 200
stockwise alert add AAPL -m us breakout-high 52
stockwise alert add HDFCBANK change-below 3   # a fall of 3% or more today
stockwise alert add TCS volume-spike 2        # 2x the 20-day average volume
stockwise alert add TCS results-in 7          # results due within a week
stockwise alert watch                 # check every 60 s while markets are open
stockwise alert watch --interval 30   # every 30 s (minimum 10)
stockwise alert watch --once          # one pass over every alert, for cron
```

`alert watch` checks each alert only while its market is open — `.NS`/`.BO`
symbols on the NSE clock, US tickers on the NYSE clock, crypto and FX
(`BTC-USD`, `USDINR=X`) around the clock — and sleeps until the next open
otherwise. Alerts are re-read every pass, so `alert add` in another terminal
takes effect without a restart.

Technical conditions (RSI, moving-average crosses, breakouts, day change,
volume spikes) are evaluated on daily bars, fetched once per symbol per check
and brought up to the live price. A cross fires on the session it happens in,
not on the days after. `results-in` reads the next results date from
`stockwise calendar` (Yahoo, plus NSE board meetings for Indian stocks,
cached for a day). Every condition is listed in
[commands.md](commands.md#alert-conditions).

Each trigger is announced **once**: an alert fires when its condition becomes
true, re-arms after the condition turns false again, and fires at most once a
trading day. Day-change and volume-spike alerts describe one session, so they
re-arm with each new session, using the quote's own session date. Until today's session has traded they give no reading, so a weekend or holiday does not repeat Friday's move; `results-in` re-arms once the results date has
passed. That
memory lives in `alert_watch.json`, so a restart doesn't repeat
notifications. If every configured channel fails, the trigger is not marked
and the next check tries again. Network errors are retried with backoff;
Ctrl+C stops it.

---

## Notifications

`alert watch` and `daemon intraday` send to every channel you configure.
All channels are opt-in, set with environment variables:

| Channel | Variables |
|---------|-----------|
| Telegram | `STOCKWISE_TELEGRAM_TOKEN` (from @BotFather), `STOCKWISE_TELEGRAM_CHAT_ID` |
| Slack | `STOCKWISE_SLACK_WEBHOOK` — an incoming-webhook URL |
| Discord | `STOCKWISE_DISCORD_WEBHOOK` — a channel webhook URL |
| Email | `STOCKWISE_SMTP_HOST`, `STOCKWISE_SMTP_USER`, `STOCKWISE_SMTP_PASSWORD`, `STOCKWISE_EMAIL_TO` (comma-separated); optional `STOCKWISE_EMAIL_FROM`, `STOCKWISE_SMTP_PORT`, `STOCKWISE_SMTP_TLS` (`starttls` — default, port 587 — `tls` for 465, or `none` for a server on this machine) |
| Desktop | `STOCKWISE_DESKTOP_NOTIFY=1`, in a build with the `desktop-notify` feature: `cargo install stockwise --features desktop-notify` |

The same settings can live in the [config file](commands.md#configuration-file)
under `[notify]`, named without the `STOCKWISE_` prefix in lower case
(`telegram_chat_id = "123"`, `smtp_host = "smtp.gmail.com"`). An environment
variable wins over the file. If you put a token or password there, keep the
file private (`chmod 600`); `config show` and `doctor` warn otherwise.

```bash
stockwise notify          # which channels are configured (secrets are never shown)
stockwise notify test     # send a test message to each; exits 1 if any fails
```

Security: the bot token, webhook URLs and SMTP password are only sent to their
own endpoint. Redirects are not followed, error messages are stripped of
URLs, and `notify` shows only the chat id, the webhook's host, or the SMTP
server. Email uses TLS (rustls); `STOCKWISE_SMTP_TLS=none` with a password is
refused unless the server is `localhost`. A failed notification is printed
as a warning and never stops the daemon or the alert loop.

Telegram setup: message @BotFather → `/newbot` → copy the token; send your
bot a message, then open `https://api.telegram.org/bot<TOKEN>/getUpdates` in
a browser and copy `chat.id`.
