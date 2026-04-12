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
