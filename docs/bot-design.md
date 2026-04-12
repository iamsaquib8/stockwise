# Bot Design & Architecture

## Current State vs Vision

### What exists today

The bots are **on-demand scanners** — you run a command, it scans once, gives suggestions, and exits. No continuous monitoring.

```
User runs command → Scan → Score → Display → Exit
```

### What should exist

Two persistent background workers that run throughout the day:

```
┌─────────────────────────────────────────────────────┐
│  INTRADAY WORKER (9:00 AM – 3:30 PM IST)            │
│                                                      │
│  9:00  Pre-market: scan overnight gaps, sector heat  │
│  9:15  Market open: lock initial positions            │
│  9:15–10:00  Aggressive phase: momentum entries       │
│  10:00–2:00  Monitoring: trail stops, book T1 profits │
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
│  4:02     Check portfolio: rebalance needed?          │
│  4:03     Check alerts: any triggered?                │
│  4:04     Tax harvest check: any losses to book?      │
│  4:05     Generate AI report → save to file           │
│  4:06     Snapshot portfolio value (wealth tracker)    │
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
├── Scan for overnight gaps in all 30 stocks
├── Check global cues (US futures, SGX NIFTY)
├── Identify watchlist candidates (top 10 by score)
└── Print morning brief

MARKET OPEN (9:15 – 9:30)
├── Run 9-strategy engine on all candidates
├── Filter by confidence (HIGH/MEDIUM only)
├── Size positions via Kelly criterion
├── Lock trades (entry, T1, T2, stop-loss, trailing stop)
├── Place orders via Angel One (or record in simulator)
└── Print trade plan

MONITORING LOOP (9:30 – 3:00, every 60s)
├── Fetch live prices for all open positions
├── For each position:
│   ├── If price >= T1: book 50%, move stop to entry (breakeven)
│   ├── If price >= T2: book remaining, position closed
│   ├── If price <= stop: exit, record loss
│   ├── Update trailing stop (max of previous, price - 1.5*ATR)
│   └── Log P&L snapshot
├── Check for new signals (only in first 2 hours)
├── Print status table every 5 minutes
└── Alert on any triggered stop or target

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
      │        │ T1 hit / stop hit
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
| Max 1% risk per trade | Kelly sizing capped, stop-loss enforced |
| Max 30% capital in one stock | Position size limit |
| Max 5 concurrent positions | Hard limit in trade manager |
| No new entries after 2 PM | Time-based gate |
| Square off by 3:15 PM | Forced exit at market price |
| No trading on circuit-breaker days | Check if NIFTY moved >3% pre-market |
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
├── Calculate current allocation %
├── Compare to target allocation
├── Flag: over-allocated (>25% in one stock)
├── Flag: under-allocated (should add more)
├── Flag: tax-loss harvesting opportunities
└── Generate rebalance suggestions

ALERT CHECK (4:04 PM)
├── Check all price alerts
├── Check if any holding hit 52-week low (accumulate signal)
├── Check if any holding hit 52-week high (trim signal)
└── Check earnings growth deceleration warnings

REPORT (4:05 PM)
├── Generate AI investment memo via Ollama
├── Save report to reports/ folder with date
├── Print summary to terminal
└── Snapshot portfolio value in wealth_history.json

WEEKLY DEEP SCAN (Sundays)
├── Run full 6-pillar scoring on extended universe (50+ stocks)
├── Cross-check with backtest results (which strategies work on which stocks)
├── Sector rotation analysis (which sectors to overweight/underweight)
├── Generate weekly investment newsletter via AI
└── Update SIP allocation recommendations
```

### Decision Matrix

```
Score > 75 + Moat Wide     → STRONG BUY (allocate 25% of monthly SIP)
Score > 65 + Moat Narrow   → BUY (allocate 20%)
Score > 55                 → HOLD (keep existing, don't add)
Score > 45                 → WATCH (on watchlist, don't buy yet)
Score < 45                 → AVOID
Score < 35 + Holding       → CONSIDER SELLING (check tax impact first)
```

### SIP Strategy

```
Every month (1st trading day):
├── Check this month's SIP budget
├── Get latest scores for top 5 stocks
├── Allocate budget by score weight
├── For each stock:
│   ├── If price < 200-day MA: invest 120% of allocation (buy the dip)
│   ├── If price > 200-day MA by 20%+: invest 80% (don't chase)
│   └── Else: invest 100% as planned
├── Place orders via Angel One
└── Record in portfolio.json
```

---

## Implementation Plan

### Phase 1: Daemon Mode (what to build)

```bash
stockwise daemon intraday          # Run intraday worker
stockwise daemon longterm          # Run longterm daily worker
stockwise daemon --all             # Run both
```

Implementation:
- Tokio async loop with `tokio::time::interval`
- Intraday: 60-second tick during market hours (9:00–3:30 IST)
- Longterm: single run at 4:00 PM, cron-scheduled or manual
- State persisted to `~/.local/share/stockwise/daemon_state.json`
- Ctrl+C graceful shutdown (square off any open positions first)

### Phase 2: Trade Execution

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

### Phase 3: Notifications

- Terminal bell on trade entry/exit
- Optional webhook to Telegram/Discord
- Daily P&L summary email (via sendgrid or local)

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

## File Structure (proposed)

```
src/
  daemon.rs           Daemon loop, market hours detection, state machine
  trade_manager.rs    Position tracking, partial exits, trailing stops
  risk_manager.rs     Daily loss limit, position limits, kill switch
  scheduler.rs        Cron-like scheduling for longterm worker
  notifier.rs         Terminal alerts, optional webhook
```

---

## Market Hours Reference

| Market | Open | Close | Pre-market |
|--------|------|-------|------------|
| NSE/BSE | 9:15 AM IST | 3:30 PM IST | 9:00 AM |
| NYSE/NASDAQ | 9:30 AM ET | 4:00 PM ET | 4:00 AM |

---

## FAQ

**Q: Should the bot trade automatically?**
A: Not yet. Start with paper trading (`sim` commands). Once you have 20+ days of HIGH confidence rating from `sim history`, then consider `--auto-trade` mode.

**Q: How much capital should I start with?**
A: Paper trade with the same amount you'd use for real. ₹25,000–₹50,000 is a good starting point for intraday. For long-term SIP, whatever your monthly budget is.

**Q: What if the bot loses money?**
A: The 1% per-trade risk limit means max loss per trade is ₹250 on ₹25K capital. Daily loss limit of 3% = ₹750 max. The kill switch stops all trading if this is breached.

**Q: Can I run both workers simultaneously?**
A: Yes, they're independent. Intraday worker runs during market hours, longterm worker runs after close. No conflict.
