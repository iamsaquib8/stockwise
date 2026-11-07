# Set up alerts and the daemon

This guide makes StockWise tell you when a price, RSI level or result date
needs your attention, and shows how to leave it running. It takes about ten
minutes.

You need: StockWise installed. For phone or email messages you also need a
Telegram bot, a Slack or Discord webhook, or an SMTP account.

## 1. Build a watchlist

```bash
stockwise watch add INFY HDFCBANK
stockwise watch
```

`watch` prints your list with live prices. The list also feeds the
[calendar](commands.md#results-and-corporate-calendar) and the MCP server.

## 2. Add alerts

An alert is a symbol, a condition and a number.

```bash
stockwise alert add RELIANCE below 1200
stockwise alert add INFY rsi-below 30              # RSI(14) at or below 30
stockwise alert add RELIANCE cross-above-sma 200   # close crosses above the 200-day average
stockwise alert add SBIN breakout-high 52          # new 52-week high
stockwise alert add HDFCBANK change-below 3        # a fall of 3% or more today
stockwise alert add TCS volume-spike 2             # 2x the 20-day average volume
stockwise alert add TCS results-in 7               # results due within a week
stockwise alert add AAPL -m us below 180           # US stock
```

Each command confirms the alert and gives it a number:

```text
  ✓ Alert #0: RELIANCE.NS below 1200.00
  ✓ Alert #1: INFY.NS RSI(14) below 30
```

The full list of conditions is in
[Alert conditions](commands.md#alert-conditions).

List the alerts, and check them once against live data:

```bash
stockwise alert                 # your alerts, in words
stockwise alert check           # each alert with its current value and status
stockwise alert remove 0        # delete alert #0
```

`alert check` marks each alert `triggered` or `waiting`. It shows `no_data` in
`--json` when there is too little history.

## 3. Choose where messages go

Channels are opt-in and set with environment variables. Check what is
configured:

```bash
stockwise notify
```

```text
  · Telegram  not configured
  · Slack     not configured
  · Discord   not configured
  · Email     not configured
  · Desktop   not configured
```

Add the variables for one channel to your shell profile or your scheduler. For
Telegram:

```bash
export STOCKWISE_TELEGRAM_TOKEN=<token from @BotFather>
export STOCKWISE_TELEGRAM_CHAT_ID=<your chat id>
```

To find the chat id, send your bot a message and open
`https://api.telegram.org/bot<TOKEN>/getUpdates` in a browser. Copy the number
under `chat` and `id`.

Then send a test message to each configured channel:

```bash
stockwise notify test
```

It exits with status 1 if any channel fails. Slack, Discord, email and desktop
settings are in [Notifications](bots.md#notifications). A token or password in
the [config file](commands.md#configuration-file) needs a private file
(`chmod 600`).

## 4. Keep checking during market hours

```bash
stockwise alert watch                 # every 60 seconds while the market is open
stockwise alert watch --interval 30   # every 30 seconds (minimum 10)
```

`alert watch` checks an alert only while its market is open. It uses the NSE
clock for Indian stocks and the NYSE clock for US stocks, and sleeps otherwise.
It re-reads your alerts on every pass, so an `alert add` in another terminal
takes effect without a restart.

Each trigger is sent once. An alert fires when its condition becomes true, and
fires again only after the condition turns false and true again, at most once a
trading day. Press Ctrl+C to stop.

### Run it from cron instead

If you would rather not keep a terminal open, run one pass on a schedule:

```bash
# every 15 minutes from 09:15 to 15:30 IST, Monday to Friday
*/15 9-15 * * 1-5  STOCKWISE_TELEGRAM_TOKEN=... STOCKWISE_TELEGRAM_CHAT_ID=... stockwise alert watch --once
```

`--once` checks every alert one time and exits. It remembers what it already
sent, so the same trigger is not repeated. Cron uses the machine's clock, so
check that your server is on IST or adjust the hours.

## 5. Run the paper-trading daemon (optional)

The daemon runs the intraday paper bot on its own and records the results. It
**never sends broker orders**.

```bash
stockwise daemon intraday 25000         # NSE hours, ₹25,000 of paper capital
stockwise daemon intraday 25000 --once  # one session, then exit (for cron)
stockwise sim history                   # results of settled sessions
```

The daemon follows the exchange clock, skips weekends and holidays, enters
positions in the first 15 minutes, checks them every 60 seconds and squares
everything off 15 minutes before the close. Ctrl+C closes open positions at the
last price, saves the day and exits. If configured, it sends entries, exits and
an end-of-day summary to the same notification channels.

There is also a once-a-day research pass:

```bash
stockwise daemon longterm   # scores 30 stocks, snapshots wealth, checks alerts, lists holdings below cost
```

For how the bots decide and the risk limits, see [Bots and alerts](bots.md) and
[Bot design](bot-design.md).

## Next steps

- [Check a stock or fund fast](guide-research.md) when an alert fires.
- [Backtest and practise on past markets](guide-practice.md) to test a rule
  before you rely on it.
- Alerts and notifications in full: [Bots and alerts](bots.md#price-and-technical-alerts)
  and [Notifications](bots.md#notifications). Command options are in
  [Portfolio](commands.md#portfolio) (`watch`, `alert`).
- Messages not arriving? See [Troubleshooting](troubleshooting.md).
