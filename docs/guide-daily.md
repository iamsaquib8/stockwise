# See what's hot and what changed today

This guide covers three daily views of your investments: `stockwise today`
(what moved and why), `stockwise hot` (what stands out, in your holdings and
the market) and the after-close digest. It ends with how to get them without
asking: the background worker and a Telegram bot.

You need: a portfolio, a watchlist, or both. See
[Import your trades](guide-portfolio.md) and
`stockwise watch add HINDUNILVR MARUTI`. The market lists work without
either.

These views describe the day's data. None of them is a recommendation.

## 1. What changed today, and why

```bash
stockwise today
```

Output from a sample portfolio after the close on 20 Nov 2026 (trimmed; the
per-row action and the headlines are cut):

```text
  Today — Sat 21 Nov 19:51 IST (Closed)
  Portfolio ₹537,572  day +₹4,616 (+0.87%)
  Market: NIFTY 50 +2.3% · SENSEX +2.3% · BANK NIFTY +4.2% · NIFTY SMALLCAP 100 -0.1% · INDIA VIX +11.2%

  Holdings by day impact
  ────────────────────────────────────────
  TCS.NS           +1.43%      +₹838  with the market (+2.3%)
  BHARTIARTL.NS    +0.83%      +₹496  with the market (+2.3%)
  RELIANCE.NS      +0.14%       +₹86  little changed, no news

  Watchlist
  ────────────────────────────────────────
  SUNPHARMA.NS     -0.79%  little changed, no news  (1.5σ)

  News feed — last 2 days, by weight × importance
  ...
```

Each holding shows its day move, its day P&L and one reason: the most
important fresh headline or exchange/SEC filing, "with the market" for an
ordinary move, or "unexplained" for an unusual move (2.5σ or more against the
benchmark) with no news found. σ is the move against the benchmark in units
of the stock's own volatility; `1.8× vol` is volume against its 20-day
average. Below the holdings come the watchlist, the market backdrop and one
news feed for both, ranked by portfolio weight × importance.

```bash
stockwise today --days 3      # news and filings from the last three days
stockwise today --no-news     # faster: moves only
stockwise today --limit 30    # a longer news feed
stockwise today --json
```

## 2. What's hot

```bash
stockwise hot
```

`hot` starts with your holdings and watchlist ranked by **heat**: the excess
move in σ, a volume surge, fresh news or filings, and a date within three
days, scaled up by portfolio weight. A large holding with a modest move can
rank above a small one with a big move. Names with no heat are left out.

Then the market (trimmed, from the same day):

```text
  Biggest moves
  Unusual volume (RVOL, paced for the time of day)
  Gaps at the open
  52-week highs
  52-week lows
  Most mentioned in today's news
  Sectors
  ────────────────────────────────────────
  Auto                    +5.1%
  Bank                    +4.2%
  PSU Bank                +3.5%
  ...
  Themes
  FII / DII flows — 20-Nov-2026 (₹ crore)
  NSE bulk and block deals
  F&O ban list (no new positions today)
  Open-interest build-up
  Cross-market movers (India and US large caps)

  → Heat = |excess move| in σ + volume surge + fresh news/filing importance + a date within 3 days, scaled by portfolio weight. Lists describe today's data; none is a recommendation.
```

FII/DII flows, bulk and block deals, the F&O ban list and open-interest
build-up are India only. When a source fails (NSE often refuses automated
requests), `hot` names it and shows the rest.

```bash
stockwise hot --cap small          # small caps only, in your lists and the market's
stockwise hot --universe nifty500  # scan one NSE index
stockwise hot --limit 4            # rows per list (default 8)
stockwise hot -m us --json
```

## 3. The after-close digest

```bash
stockwise digest
stockwise digest --send     # also send it to your notification channels
```

The digest is one message: the day's P&L, the market, the top contributors
and detractors with the news or filing behind each move, monitor signals
(below cost, off the 52-week high, dates tomorrow), alerts that triggered and
dates coming up. For example (trimmed):

```text
  StockWise digest 2026-11-20: +₹4,616 (+0.87%)
Portfolio ₹537,572

Top contributors
• TCS.NS +1.4% +₹838 — with the market (+2.3%)

Monitor
• INFY.NS is 27% below your cost — 1098.00 against an average cost of 1500.00.
• TCS.NS is 29% off its 52-week high — 2376.20 against a closing high of 3324.90 (22 Dec 2025).
```

Set up a channel first (Telegram, Slack, Discord, email, ntfy, Pushover or
desktop): see [Set up alerts](guide-alerts.md#3-choose-where-messages-go) and
check it with `stockwise notify test`.

## 4. Get it every day without asking

The background worker watches every holding (big drops, new 52-week lows,
gaps, falls from cost or the peak, dates tomorrow, pledge, shareholding,
rating and insider changes) and sends the digest after the close.

```bash
stockwise worker start            # until you stop it or reboot
stockwise worker install          # start it at every login (launchd or systemd user service)
stockwise worker install --dry-run
stockwise worker status
stockwise monitor                 # thresholds and recent signals
stockwise monitor run --notify    # one pass now, sending what is new
```

Keep the night quiet and route by priority:

```bash
export STOCKWISE_QUIET_HOURS=22:00-07:00
export STOCKWISE_NOTIFY_ROUTES=desktop=high,telegram=low
```

Thresholds live under `[monitor]` in the config file. See
[Holding monitor and digest](commands.md#holding-monitor-and-digest).

## 5. Ask from your phone

With the Telegram token and chat id set (see
[notifications](bots.md#notifications)), run the bot:

```bash
stockwise telegram          # answers until you stop it
stockwise telegram --once   # answer pending messages once and exit, for cron
```

It answers `/today`, `/hot`, `/review` (the rule-based action for each
holding; see [Review your holdings](guide-actions.md)) and `/digest` from your
chat, and ignores every other chat. `STOCKWISE_DIGEST_TIME=16:15` also sends
the digest every trading day.

## 6. A one-line view in your status bar

```bash
stockwise statusbar --format waybar
```

`statusbar` prints the portfolio's day change and a few quotes on one line for
tmux, waybar, sketchybar or starship. Snippets are in
[Status bars](commands.md#status-bars).

## Next steps

- What the rules say about each holding:
  [Review your holdings with rule-based actions](guide-actions.md).
- Price and indicator alerts: [Set up alerts and the daemon](guide-alerts.md).
- Ask an assistant: the MCP tools `today`, `news_feed` and `digest` return
  the same data. See [Use StockWise from Claude, Gemini, Codex or Cursor](guide-mcp.md).
- Every option: [What's hot and today](commands.md#whats-hot-and-today).
- Messages not arriving, or a source failing: [Troubleshooting](troubleshooting.md).
