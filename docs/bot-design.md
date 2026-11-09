# Bot Design & Architecture

## Current State vs Vision

### What exists today

- **On-demand scanners** (`intraday`, `longterm`): scan once, show the scores, exit.
- **`daemon intraday`**: a paper-only worker on the exchange clock (IST /
  ET, holidays included) that enters at the open, manages positions every
  60 seconds, squares off before the close and settles into the simulator
  history. It survives network errors, saves its state every tick
  (`daemon_state.json`), resumes after a restart, and squares off on Ctrl+C.
- **`daemon longterm`**: a one-shot post-market scan.
- **`alert watch`**: checks price alerts while their market is open.
- **Notifications** to Telegram, Slack, Discord, email and the desktop for
  alert triggers and daemon entries, exits, kill switch and end of day.

Not built: live order execution from the daemon (Phase 2), the learning loop
(Phase 4), and scheduling (run the workers from cron/systemd/launchd).

### Design

Two persistent background workers that run throughout the day:

```
┌─────────────────────────────────────────────────────┐
│  INTRADAY WORKER (9:00 AM – 3:30 PM IST)            │
│                                                      │
│  9:00  Pre-market: scan overnight gaps, sector heat  │
│  9:15  Market open: lock initial positions            │
│  9:15–10:00  Aggressive phase: momentum entries       │
│  10:00–2:00  Monitoring: trail exit levels, book level 1 │
│  2:00–3:00  Wind-down: no new entries, tighten stops  │
│  3:15  Square off: close all intraday positions       │
│  3:30  EOD: settle, record P&L, update sim history    │
│                                                      │
│  Runs every 60 seconds during market hours            │
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│  LONGTERM WORKER (runs daily after market close)     │
│                                                      │
│  4:00 PM  Scan all stocks, score on 6 pillars        │
│  4:01     Run Monte Carlo on top candidates           │
│  4:02     Portfolio value, wealth snapshot            │
│  4:03     Check alerts: any triggered?                │
│  4:04     Holdings priced below their average cost    │
│  4:05     AI research notes (optional)                │
│                                                      │
│  Runs once daily, optionally weekly deep scan         │
└─────────────────────────────────────────────────────┘
```

---

## Intraday Worker: Detailed Design

### Lifecycle

```
PRE-MARKET (9:00 – 9:14)
├── Fetch sector heat (NIFTY IT, Bank, Pharma, etc.)
├── Scan for overnight gaps across the market's universe
├── Check global cues (US futures, SGX NIFTY)
├── Identify watchlist candidates (top 10 by score)
└── Print morning brief

MARKET OPEN (9:15 – 9:30)
├── Run 9-strategy engine on all candidates
├── Filter by confidence (HIGH/MEDIUM only)
├── Size positions via Kelly criterion
├── Lock paper positions (opening price, level 1, level 2, exit level, trailing exit level)
├── Record them in the simulator (the daemon never places orders)
└── Print the positions

MONITORING LOOP (9:30 – 3:00, every 60s)
├── Fetch live prices for all open positions
├── For each position:
│   ├── If price >= level 1: book 50%, move exit level to the opening price
│   ├── If price >= level 2: book remaining, position closed
│   ├── If price <= exit level: close, record loss
│   ├── Update trailing exit level (max of previous, price - 1.5*ATR)
│   └── Log P&L snapshot
├── Check for new signals (only in first 2 hours)
├── Print status table every 5 minutes
└── Notify when level 1, level 2 or the exit level is reached

WIND-DOWN (3:00 – 3:15)
├── No new entries
├── Tighten all trailing stops to 1x ATR
├── Book any position with >0.5% profit
└── Prepare for square-off

SQUARE OFF (3:15 – 3:20)
├── Close all remaining positions at market
├── Calculate final P&L
├── Settle simulation session
└── Generate EOD report

POST-MARKET (3:30+)
├── Save results to sim_history.json
├── Update wealth tracker
├── Generate AI EOD report
└── Print daily scorecard
```

### State Machine

```
          ┌──────────┐
          │  IDLE     │ (before 9:00 or after 3:30)
          └────┬─────┘
               │ 9:00 AM
          ┌────▼─────┐
          │ PRE_SCAN  │ gaps, sectors, candidates
          └────┬─────┘
               │ 9:15 AM
          ┌────▼─────┐
          │ ENTERING  │ place orders, lock trades
          └────┬─────┘
               │ orders filled
          ┌────▼─────┐
      ┌──►│ WATCHING  │◄──── main loop (60s ticks)
      │   └────┬─────┘
      │        │ level 1 / exit level
      │   ┌────▼─────┐
      └───┤ MANAGING  │ partial exits, trail stops
          └────┬─────┘
               │ 3:15 PM
          ┌────▼─────┐
          │ CLOSING   │ square off all
          └────┬─────┘
               │ done
          ┌────▼─────┐
          │ SETTLED   │ record P&L, generate report
          └──────────┘
```

### Data Flow

```
Yahoo Finance (prices) ──┐
                         ├──► Intraday Engine ──► Trade Manager ──► Angel One API
Ollama (AI insights) ────┘         │                    │
                                   │                    ▼
                              Score/Signal         Order Placed
                                   │                    │
                                   ▼                    ▼
                              sim_history.json     Real P&L
```

### Risk Controls

| Rule | Implementation |
|------|---------------|
| Max 1% risk per trade | Kelly sizing capped, exit level enforced |
| Max 30% capital in one stock | Position size limit |
| Max 5 concurrent positions | Hard limit in trade manager |
| Total allocation ≤ capital | Plans are shrunk or dropped, best first, until they fit |
| No new entries after the opening window | Entries only 9:15–9:30; nothing after |
| Square off by 3:15 PM | Forced exit at market price |
| No trading on circuit-breaker days | *Planned:* check if NIFTY moved >3% pre-market |
| No trading on holidays | Exchange holiday table (`src/clock.rs`) |
| Daily loss limit (3% of capital) | Kill switch — close all if breached |

---

## Long-Term Worker: Detailed Design

### Daily Run (after market close)

```
SCAN (4:00 PM)
├── Fetch quotes for all 30 popular stocks
├── Fetch 1Y daily charts for each
├── Score on 6 pillars (val, growth, quality, momentum, dividend, safety)
├── Run Monte Carlo (1000 sims × 5 years)
├── Detect moat rating
└── Rank by total score

PORTFOLIO CHECK (4:02 PM)
├── Fetch current holdings from portfolio.json
├── Value them in the market's currency
└── Snapshot the value in wealth_history.json (skipped when a price is missing)

ALERT CHECK (4:03 PM)
└── Check all price alerts

HOLDINGS BELOW COST (4:04 PM)
└── List holdings whose price is under their average cost

REPORT (4:05 PM)
└── Optional AI research notes via Ollama (score, data coverage, moat signals,
    risk level), printed to the terminal
```

Not built: weekly deep scan, saved reports, allocation or rebalance output.
`longterm` and `daemon longterm` show scores and the facts behind them. They do
not allocate a budget, rank names as picks, place orders or say when to act.

### Score bands

The composite and each pillar use the same descriptive bands as `verdict`. A
band describes the score; it is not an instruction.

```
Score 65 and above   Strong
Score 45 to 64       Moderate
Score below 45       Weak
No inputs            No data (the composite shows n/a under 50% data coverage)
```

### SIP arithmetic

`longterm` compounds your monthly budget at an *assumed* annual return that is
set from the composite score (a rule of thumb, not a forecast) and shows the
monthly amount that would reach 10 lakh / 1 million in 10 years at that rate.
Every instalment grows only for the time it stays invested. It places no
orders and does not size a position.

---

## Implementation Plan

### Phase 1: Daemon Mode (what to build)

```bash
stockwise daemon intraday          # Run intraday worker
stockwise daemon longterm          # Run longterm daily worker
stockwise daemon --all             # Run both
```

Implementation (done):
- Tokio loop; the phase comes from `clock::market_phase(market, now)` — IST
  for NSE, US Eastern (with daylight saving) for NYSE, holidays closed
- Intraday: 60-second tick during market hours; one Yahoo session reused,
  failed ticks retried with bounded backoff (15 s → 5 min), never fatal
- State persisted to `daemon_state.json` (`daemon_state_us.json` for NYSE)
  after every tick and resumed on a same-day restart; a day whose square-off
  the daemon missed is handed to `sim settle`
- Ctrl+C / SIGTERM: square off at last prices, settle into the simulator
  history, save, exit 0
- Longterm: single run, cron-scheduled or manual
- Not built: `daemon --all`

### Phase 2: Trade Execution (planned — not implemented)

> Today the daemon is **paper-only**: it never sends orders to a broker. Live
> orders are placed only by `stockwise trade buy/sell/limit`, one at a time,
> after a preview and confirmation.

```
Signal Generated ──► Confirmation Check ──► Angel One API
                          │
                     Risk check:
                     - Within daily loss limit?
                     - Position count < 5?
                     - Not in wind-down period?
                     - Capital available?
```

- Auto-execute: `stockwise daemon intraday --auto-trade`
- Paper mode (default): `stockwise daemon intraday --paper`
- Requires Angel One configured

### Phase 3: Notifications (done)

`src/notify.rs` sends to Telegram (bot API), Slack and Discord (incoming
webhooks), email (SMTP via lettre + rustls) and the desktop (notify-rust,
behind the `desktop-notify` feature). Channels are configured with
environment variables; `stockwise notify test` checks them. The daemon sends
openings, closings (level 1, level 2, exit level, square-off), the kill switch and the
end-of-day summary; `alert watch` sends each trigger once. See
[bots.md](bots.md#notifications).

### Phase 4: Learning Loop

```
After 30+ simulation days:
├── Analyze which strategies win most
├── Adjust strategy weights automatically
├── Identify which market regimes favor which strategies
├── Backtest adjusted weights on historical data
└── If improvement > 5%: adopt new weights
```

---

## File Structure

```
src/
  clock.rs            Exchange clock: IST / US Eastern, holiday tables, phases
  daemon.rs           Positions, risk manager, daemon state + events, shutdown
  commands/worker.rs  The intraday and long-term loops
  commands/alerting.rs  `alert watch` and `notify`
  notify.rs           Telegram, Slack, Discord, email, desktop
  simulator.rs        Sessions, stats, and the bar replay behind `sim settle`
```

Planned: `scheduler.rs` (cron-like scheduling for the long-term worker).

---

## Market Hours Reference

| Market | Open | Close | Pre-market |
|--------|------|-------|------------|
| NSE/BSE | 9:15 AM IST | 3:30 PM IST | 9:00 AM |
| NYSE/NASDAQ | 9:30 AM ET | 4:00 PM ET (1:00 PM on early-close days) | 4:00 AM |

Holidays: NSE 2026 (official), 2027 (provisional), NYSE 2026–2028. To add a
year, see the module docs at the top of `src/clock.rs`.

---

## FAQ

**Q: Should the bot trade automatically?**
A: Not yet. Start with paper trading (`sim` commands). Once you have 20+ days of HIGH confidence rating from `sim history`, then consider a future auto-trade mode (not built yet — today the daemon only paper-trades).

**Q: How much capital should I start with?**
A: Paper trade with the same amount you'd use for real. ₹25,000–₹50,000 is a good starting point for intraday. For long-term SIP, whatever your monthly budget is.

**Q: What if the bot loses money?**
A: The 1% per-trade risk limit means max loss per trade is ₹250 on ₹25K capital. Daily loss limit of 3% = ₹750 max. The kill switch stops all trading if this is breached.

**Q: Can I run both workers simultaneously?**
A: Yes, they're independent. Intraday worker runs during market hours, longterm worker runs after close. No conflict.
