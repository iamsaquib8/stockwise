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
- Smart stop-loss: uses support levels when available, else ATR-based

### Output

- Sector heat map (which sectors are moving)
- Market regime detection (Strong Uptrend → Strong Downtrend)
- Top 15 signals ranked by score
- Top 5 trade plans with: dual targets (T1: book 50%, T2: ride), trailing stop, Kelly fraction

---

## Long-Term Wealth Bot

6-pillar fundamental scoring with Monte Carlo simulation.

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
| Momentum | 10% | Golden/Death cross, analyst consensus, target upside |
| Dividend | 10% | Yield level, DRIP compounding |
| Safety | 15% | Beta, market cap, historical volatility, debt levels |

### Moat Analysis

Stocks are rated Wide/Narrow/No Moat based on:
- Profit margins >25% (pricing power)
- ROE >25% (capital efficiency)
- Revenue growth >25% (market dominance)
- Market cap >$1T (scale advantage)

### Monte Carlo Simulation

- 1000 simulated 5-year paths using historical volatility
- Reports: P10 (worst case), Median, P90 (best case) returns
- Uses deterministic PRNG for reproducibility

### Output

- Top 12 stocks ranked across all 6 pillars
- Detailed top 3: moat rating, risk tier, PEG, earnings yield
- 5Y and 10Y projections with DRIP compounding
- SIP calculator (how much/month for ₹10L in 10 years)
- Suggested allocation across top 5 stocks (score-weighted)

---

## Paper Trading Simulator

Test the intraday bot without risking real money.

```bash
stockwise sim start 25000 2    # Morning: lock in trades
stockwise sim status           # During day: live P&L
stockwise sim settle           # EOD: record results
stockwise sim history          # Performance over time
```

### What It Tracks

- Per trade: entry/exit, target/stop hit, P&L, strategies used
- Per day: total P&L, win/loss count, target achievement
- Cumulative: equity curve, Sharpe ratio, win streaks, day/trade win rates

### Confidence Rating (after 5+ days)

| Rating | Criteria |
|--------|----------|
| HIGH | >65% day win rate + >55% trade win rate + profitable |
| MODERATE | >50% day win rate + profitable |
| LOW | Below moderate threshold |

---

## Daemon Mode

Persistent background workers that run the bots continuously.

### Intraday Daemon

```bash
stockwise daemon intraday 25000    # Run all day with ₹25K
```

Lifecycle:
```
9:00 AM   Pre-market scan (sectors, gaps)
9:15 AM   Enter top 5 positions (9 strategies, Kelly sizing)
9:15–3:00 Monitor every 60 seconds:
          - T1 hit → book 50%, move stop to breakeven
          - T2 hit → close fully
          - Stop hit → exit, record loss
          - Trailing stop updates (1.5% trail)
          - Wind-down (2 PM+): tighten stops to 0.5%
3:15 PM   Square off all remaining positions
3:30 PM   Settle → save to sim_history.json → AI EOD report
```

Risk controls:
- 1% max risk per trade
- 3% daily loss kill switch (closes everything)
- Max 5 concurrent positions
- No new entries after 2 PM
- Forced square-off at 3:15 PM

Results are saved to simulation history — check with `stockwise sim history`.

### Long-Term Daemon

```bash
stockwise daemon longterm
```

Runs 5 steps:
1. Score 30 stocks on 6 pillars + Monte Carlo
2. Check portfolio value, snapshot wealth tracker
3. Check all price alerts
4. Scan for tax-loss harvest opportunities
5. Generate AI investment memo via Ollama

Run daily after market close, or weekly for deep analysis.

### Deep Dive

One command for everything about a single stock:

```bash
stockwise deep RELIANCE
```

Shows: company info, price action, valuation (P/E, PEG, P/B, EV/EBITDA), profitability, financial health, dividends, analyst ratings, 1Y chart, all technicals (RSI + gauge, SMA, MACD, Bollinger, ATR, VWAP), support/resistance, Fibonacci, risk (vol, Sharpe, drawdown, VaR), OBV trend, open gaps, insights, verdict, and AI analysis.
