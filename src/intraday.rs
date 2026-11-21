use crate::api::{Quote, YahooClient};
use crate::market;
use crate::technical;
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate};

// ══════════════════════════════════════════════════════════
// MULTI-STRATEGY INTRADAY ENGINE
// ══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Long,
    Short,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Long => write!(f, "long"),
            Direction::Short => write!(f, "short"),
        }
    }
}

impl Direction {
    /// True for a stored direction string. Files written before 0.8.1 hold
    /// `BUY` / `SELL`; newer ones hold `long` / `short`. Anything else is short.
    pub fn is_long_str(s: &str) -> bool {
        s.eq_ignore_ascii_case("long") || s.eq_ignore_ascii_case("buy")
    }

    /// `long` or `short` for a stored direction string, old spelling or new.
    pub fn label_of(s: &str) -> &'static str {
        if Self::is_long_str(s) {
            "long"
        } else {
            "short"
        }
    }

    /// Which way the signal leans, for screens that only describe data.
    pub fn lean(&self) -> &'static str {
        match self {
            Direction::Long => "bullish",
            Direction::Short => "bearish",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarketRegime {
    StrongUptrend,
    Uptrend,
    Ranging,
    Downtrend,
    StrongDowntrend,
}

impl std::fmt::Display for MarketRegime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarketRegime::StrongUptrend => write!(f, "Strong Uptrend"),
            MarketRegime::Uptrend => write!(f, "Uptrend"),
            MarketRegime::Ranging => write!(f, "Ranging"),
            MarketRegime::Downtrend => write!(f, "Downtrend"),
            MarketRegime::StrongDowntrend => write!(f, "Strong Downtrend"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Confidence::High => write!(f, "HIGH"),
            Confidence::Medium => write!(f, "MEDIUM"),
            Confidence::Low => write!(f, "LOW"),
        }
    }
}

/// Individual strategy signal
#[derive(Debug, Clone)]
pub struct StrategySignal {
    pub name: &'static str,
    pub direction: Direction,
    pub strength: f64, // 0-100
    pub reason: String,
}

/// Full intraday analysis for a stock
#[derive(Debug)]
pub struct IntradaySignal {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub currency: Option<String>,
    pub change_pct: f64,
    /// Today's volume over the usual volume by this time of day; 0 when
    /// unknown (see `volume_known`).
    pub volume_ratio: f64,
    /// Whether `volume_ratio` was measured. Unknown volume lowers the score.
    pub volume_known: bool,
    pub regime: MarketRegime,
    pub confidence: Confidence,
    pub direction: Direction,
    pub score: f64,
    // Individual strategy signals
    pub strategies: Vec<StrategySignal>,
    // Raw indicators
    pub rsi: Option<f64>,
    pub vwap: Option<f64>,
    pub above_vwap: bool,
    pub macd_histogram: Option<f64>,
    pub bb_position: Option<f64>,
    pub atr_pct: Option<f64>,
    pub obv_trend: OBVTrend,
    pub gap_detected: bool,
    pub near_support: bool,
    pub near_resistance: bool,
    pub pivot: Option<f64>,
    pub support1: Option<f64>,
    pub resistance1: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OBVTrend {
    Rising,
    Falling,
    Flat,
}

impl std::fmt::Display for OBVTrend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OBVTrend::Rising => write!(f, "Accumulation"),
            OBVTrend::Falling => write!(f, "Distribution"),
            OBVTrend::Flat => write!(f, "Neutral"),
        }
    }
}

/// OBV trend against its 10-period average. The ±5% band is relative to the
/// average's magnitude: OBV is a signed running total, so `avg * 1.05` would
/// invert the comparison when the average is negative (a stock with
/// historical selling).
fn obv_trend(obv_data: &[f64]) -> OBVTrend {
    if obv_data.len() <= 10 {
        return OBVTrend::Flat;
    }
    let Some(avg) = technical::sma(obv_data, 10) else {
        return OBVTrend::Flat;
    };
    let last = *obv_data.last().unwrap();
    let diff = (last - avg) / avg.abs().max(1.0);
    if diff > 0.05 {
        OBVTrend::Rising
    } else if diff < -0.05 {
        OBVTrend::Falling
    } else {
        OBVTrend::Flat
    }
}

/// Paper trade plan: levels from the ATR, size from a fixed risk budget
/// (Kelly once the paper record is long enough)
#[derive(Debug)]
pub struct TradePlan {
    pub signal: IntradaySignal,
    pub entry: f64,
    pub target1: f64, // first target (book 50%)
    pub target2: f64, // second target (let it ride)
    pub stop_loss: f64,
    pub trailing_stop: f64,
    pub qty: u32,
    pub capital_required: f64,
    pub max_risk: f64, // max loss in currency
    pub expected_profit: f64,
    pub risk_reward: f64,
    pub kelly_fraction: f64, // half-Kelly from the measured record; 0 before it
    pub position_pct: f64,   // % of capital allocated
}

/// Sector heat — which sectors are moving
#[derive(Debug)]
pub struct SectorHeat {
    pub name: String,
    pub change_pct: f64,
    pub hot: bool,
}

// ══════════════════════════════════════════════════════════
// CORE ENGINE
// ══════════════════════════════════════════════════════════

/// A VWAP cross on this bar: the previous bar closed at or below the VWAP and
/// this one closes less than 0.5% above it (Long, a reclaim), or the mirror
/// image (Short, a rejection). The distance from the VWAP is in percent.
fn vwap_cross(prev_close: f64, price: f64, vwap: f64) -> Option<(Direction, f64)> {
    if vwap <= 0.0 {
        return None;
    }
    let dist_pct = (price - vwap) / vwap * 100.0;
    if prev_close <= vwap && dist_pct > 0.0 && dist_pct < 0.5 {
        Some((Direction::Long, dist_pct))
    } else if prev_close >= vwap && dist_pct < 0.0 && dist_pct > -0.5 {
        Some((Direction::Short, -dist_pct))
    } else {
        None
    }
}

/// Detect market regime from price action
fn detect_regime(closes: &[f64]) -> MarketRegime {
    if closes.len() < 20 {
        return MarketRegime::Ranging;
    }
    let sma10 = technical::sma(closes, 10).unwrap_or(0.0);
    let sma20 = technical::sma(closes, 20).unwrap_or(0.0);
    let price = *closes.last().unwrap();
    let rsi = technical::rsi(closes, 14).unwrap_or(50.0);

    // ADX-like trend strength using directional movement
    let returns = technical::daily_returns(closes);
    let recent = if returns.len() > 10 {
        &returns[returns.len() - 10..]
    } else {
        &returns
    };
    let positive: f64 = recent.iter().filter(|&&r| r > 0.0).count() as f64;
    let trend_strength = (positive / recent.len() as f64 - 0.5).abs() * 2.0; // 0=mixed, 1=directional

    if price > sma10 && sma10 > sma20 && trend_strength > 0.6 && rsi > 55.0 {
        MarketRegime::StrongUptrend
    } else if price > sma20 && rsi > 50.0 {
        MarketRegime::Uptrend
    } else if price < sma10 && sma10 < sma20 && trend_strength > 0.6 && rsi < 45.0 {
        MarketRegime::StrongDowntrend
    } else if price < sma20 && rsi < 50.0 {
        MarketRegime::Downtrend
    } else {
        MarketRegime::Ranging
    }
}

/// Today's session from 5-minute bars: the session VWAP and how today's
/// volume so far compares with the volume earlier sessions had traded by the
/// same time of day.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SessionStats {
    pub vwap: Option<f64>,
    /// Today's volume over the average volume of the previous sessions at
    /// the same minute of the session. None when unknown (before the open,
    /// or without enough intraday history).
    pub volume_pace: Option<f64>,
}

/// One intraday bar with every price present.
#[derive(Debug, Clone, Copy, PartialEq)]
struct IntradayBar {
    time: i64,
    high: f64,
    low: f64,
    close: f64,
    /// None when the feed left it out: never read as zero trades.
    volume: Option<u64>,
}

/// Bars of an intraday chart, aligned by index: a bar missing its high, low
/// or close is dropped as a whole (never shifted against the other columns);
/// missing volume stays missing.
fn intraday_bars(chart: &crate::api::ChartData) -> Vec<IntradayBar> {
    let Some(q) = chart.indicators.quote.first() else {
        return Vec::new();
    };
    let at = |c: &Option<Vec<Option<f64>>>, i: usize| {
        c.as_ref()
            .and_then(|v| v.get(i).copied().flatten())
            .filter(|p| p.is_finite() && *p > 0.0)
    };
    chart
        .timestamp
        .as_deref()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(i, &time)| {
            Some(IntradayBar {
                time,
                high: at(&q.high, i)?,
                low: at(&q.low, i)?,
                close: at(&q.close, i)?,
                volume: q.volume.as_ref().and_then(|v| v.get(i).copied().flatten()),
            })
        })
        .collect()
}

/// Fewest earlier sessions the volume pace is averaged over.
const MIN_PACE_SESSIONS: usize = 2;

/// [`SessionStats`] for the session dated `today` from several sessions of
/// 5-minute bars (`5d` of them from Yahoo).
fn session_stats(bars: &[IntradayBar], market: market::Market, today: NaiveDate) -> SessionStats {
    use std::collections::BTreeMap;
    let open_of = |date: NaiveDate| {
        crate::clock::hours(market, date)
            .map(|h| crate::clock::at(market, date, h.open).timestamp())
    };
    let mut days: BTreeMap<NaiveDate, Vec<&IntradayBar>> = BTreeMap::new();
    for b in bars {
        if let Some(t) = DateTime::from_timestamp(b.time, 0) {
            days.entry(crate::clock::session_date(market, t))
                .or_default()
                .push(b);
        }
    }
    let Some(session) = days.get(&today).filter(|d| !d.is_empty()) else {
        return SessionStats::default();
    };
    // Volume over a run of bars; None when any of them lacks it.
    let volume_of = |bars: &mut dyn Iterator<Item = &&IntradayBar>| -> Option<f64> {
        bars.map(|b| b.volume.map(|v| v as f64)).sum()
    };
    // VWAP over the bars whose volume is known.
    let (tp_vol, known_vol) = session
        .iter()
        .filter_map(|b| {
            b.volume
                .map(|v| ((b.high + b.low + b.close) / 3.0, v as f64))
        })
        .fold((0.0, 0.0), |(a, n), (tp, v)| (a + tp * v, n + v));
    let vwap = (known_vol > 0.0).then(|| tp_vol / known_vol);

    // Minutes into the session of today's latest bar, then each earlier
    // session's volume up to the same minute. A session (today's included)
    // with a bar of unknown volume in that span is left out; a measured
    // zero counts.
    let volume_pace = open_of(today).and_then(|open| {
        let traded = volume_of(&mut session.iter())?;
        let elapsed = session.last()?.time - open;
        let expected: Vec<f64> = days
            .range(..today)
            .filter_map(|(date, bars)| {
                let open = open_of(*date)?;
                let mut covered = bars.iter().filter(|b| b.time - open <= elapsed).peekable();
                covered.peek()?;
                volume_of(&mut covered)
            })
            .collect();
        if expected.len() < MIN_PACE_SESSIONS {
            return None;
        }
        let expected = expected.iter().sum::<f64>() / expected.len() as f64;
        (expected > 0.0).then(|| traded / expected)
    });
    SessionStats { vwap, volume_pace }
}

/// Scale on the score when the volume pace is unknown: the same as a
/// below-average day, so a missing average is never read as normal volume.
const UNKNOWN_VOLUME_MULT: f64 = 0.85;

/// Run all strategies and compute a composite signal from daily bars (about
/// three months, for the trend, RSI, MACD, Bollinger bands, ATR and OBV) and
/// today's 5-minute session (VWAP and volume pace).
#[allow(clippy::too_many_arguments)]
fn compute_signal(
    symbol: &str,
    quote: &Quote,
    price: f64,
    daily: &crate::backtest::Bars,
    session: SessionStats,
    prior: Option<crate::api::PriorSession>,
    custom: Option<StrategySignal>,
) -> IntradaySignal {
    let (closes, highs, lows, opens, volumes) = (
        &daily.close[..],
        &daily.high[..],
        &daily.low[..],
        &daily.open[..],
        &daily.volume[..],
    );
    let mut strategies: Vec<StrategySignal> = custom.into_iter().collect();
    let regime = detect_regime(closes);
    // Oversold readings (RSI, the lower Bollinger band, distance below the
    // 20-day average, accumulation on weak price) describe one thing: a dip.
    // They become one vote, as strong as the strongest reading, so a single
    // fall is not counted four times. Overbought readings likewise.
    let mut dip: Vec<(f64, String)> = Vec::new();
    let mut stretched: Vec<(f64, String)> = Vec::new();

    // ── RSI ──
    let rsi = technical::rsi(closes, 14);
    if let Some(r) = rsi {
        if r < 30.0 {
            dip.push((
                (85.0 + (30.0 - r)).min(100.0),
                format!("RSI deeply oversold at {:.0}", r),
            ));
        } else if r < 40.0 {
            dip.push((60.0, format!("RSI approaching oversold at {:.0}", r)));
        } else if r > 70.0 {
            stretched.push((
                (60.0 + (r - 70.0)).min(100.0),
                format!("RSI overbought at {:.0}", r),
            ));
        }
    }

    // ── Strategy: VWAP Reclaim (today's session VWAP) ──
    let vwap = session.vwap;
    let above_vwap = vwap.is_some_and(|v| price > v);
    let prev_close = closes.len().checked_sub(2).map(|i| closes[i]);
    if let (Some(v), Some(prev)) = (vwap, prev_close) {
        match vwap_cross(prev, price, v) {
            Some((Direction::Long, dist)) => strategies.push(StrategySignal {
                name: "VWAP Reclaim",
                direction: Direction::Long,
                strength: 75.0,
                reason: format!("Crossed above the session VWAP; now {:.2}% above", dist),
            }),
            Some((Direction::Short, dist)) => strategies.push(StrategySignal {
                name: "VWAP Rejection",
                direction: Direction::Short,
                strength: 65.0,
                reason: format!("Crossed below the session VWAP; now {:.2}% below", dist),
            }),
            _ => {}
        }
    }

    // ── Strategy: Bollinger Band Squeeze; band touches feed the families ──
    let bb = technical::bollinger_bands(closes, 20);
    let bb_position = bb.map(|(upper, _, lower)| {
        if (upper - lower).abs() < f64::EPSILON {
            0.5
        } else {
            (price - lower) / (upper - lower)
        }
    });
    if let Some((upper, middle, lower)) = bb {
        let bandwidth = (upper - lower) / middle * 100.0;
        if bandwidth < 3.0 && price > middle {
            strategies.push(StrategySignal {
                name: "BB Squeeze",
                direction: Direction::Long,
                strength: 70.0,
                reason: format!(
                    "Bollinger squeeze ({:.1}% width), price above middle",
                    bandwidth
                ),
            });
        } else if bb_position.unwrap_or(0.5) < 0.1 {
            dip.push((65.0, "Price touching lower Bollinger Band".into()));
        } else if bb_position.unwrap_or(0.5) > 0.95 {
            stretched.push((60.0, "Price at upper Bollinger Band".into()));
        }
    }

    // ── Strategy: MACD Crossover (two consecutive histograms: 36+ bars) ──
    let macd = technical::macd(closes);
    let macd_histogram = macd.map(|(_, _, h)| h);
    if closes.len() > 1 {
        let prev_macd = technical::macd(&closes[..closes.len() - 1]);
        if let (Some((_, _, hist)), Some((_, _, prev_hist))) = (macd, prev_macd) {
            if hist > 0.0 && prev_hist <= 0.0 {
                strategies.push(StrategySignal {
                    name: "MACD Cross",
                    direction: Direction::Long,
                    strength: 70.0,
                    reason: "MACD just crossed above signal line".into(),
                });
            } else if hist < 0.0 && prev_hist >= 0.0 {
                strategies.push(StrategySignal {
                    name: "MACD Cross",
                    direction: Direction::Short,
                    strength: 65.0,
                    reason: "MACD just crossed below signal line".into(),
                });
            }
        }
    }

    // ── Strategy: Volume Breakout (volume against the time-of-day pace) ──
    let volume_known = session.volume_pace.is_some();
    let volume_ratio = session.volume_pace.unwrap_or(0.0);
    if volume_ratio > 2.0 && quote.regular_market_change_percent.unwrap_or(0.0) > 0.5 {
        strategies.push(StrategySignal {
            name: "Volume Breakout",
            direction: Direction::Long,
            strength: 80.0,
            reason: format!(
                "Volume {:.1}x the usual pace for this time of day, with positive price action",
                volume_ratio
            ),
        });
    }

    // ── Strategy: Support/Resistance Bounce ──
    let (mut near_support, mut near_resistance) = (false, false);
    let (mut pivot_val, mut s1_val, mut r1_val) = (None, None, None);
    // Pivots come from the last completed session, not today's live bar.
    if let Some(p) = prior {
        let (pivot, r1, _r2, _r3, s1, _s2, _s3) = technical::pivot_points(p.high, p.low, p.close);
        pivot_val = Some(pivot);
        s1_val = Some(s1);
        r1_val = Some(r1);
        let dist_to_s1 = ((price - s1) / price * 100.0).abs();
        let dist_to_r1 = ((price - r1) / price * 100.0).abs();
        if dist_to_s1 < 0.5 && price > s1 {
            near_support = true;
            strategies.push(StrategySignal {
                name: "Support Bounce",
                direction: Direction::Long,
                strength: 72.0,
                reason: format!("Price bouncing off S1 support ({:.2})", s1),
            });
        }
        if dist_to_r1 < 0.5 && price < r1 {
            near_resistance = true;
            strategies.push(StrategySignal {
                name: "Resistance Test",
                direction: Direction::Short,
                strength: 55.0,
                reason: format!("Price hitting R1 resistance ({:.2})", r1),
            });
        }
    }

    // ── Strategy: Gap Play ──
    let gap_detected = if opens.len() >= 2 && highs.len() >= 2 && lows.len() >= 2 {
        let gaps = technical::detect_gaps(opens, highs, lows, closes);
        let recent_unfilled: Vec<_> = gaps
            .iter()
            .filter(|g| !g.filled && g.index >= opens.len().saturating_sub(5))
            .collect();
        if let Some(gap) = recent_unfilled.last() {
            match gap.gap_type {
                technical::GapType::Up => {
                    strategies.push(StrategySignal {
                        name: "Gap Fill",
                        direction: Direction::Long,
                        strength: 60.0,
                        reason: "Recent unfilled gap up — momentum continuation".into(),
                    });
                    true
                }
                technical::GapType::Down => {
                    strategies.push(StrategySignal {
                        name: "Gap Fill",
                        direction: Direction::Short,
                        strength: 55.0,
                        reason: "Recent unfilled gap down".into(),
                    });
                    true
                }
            }
        } else {
            false
        }
    } else {
        false
    };

    // ── OBV accumulation on weak price ──
    let obv_trend = obv_trend(&technical::obv(closes, volumes));
    if obv_trend == OBVTrend::Rising
        && (regime == MarketRegime::Ranging || regime == MarketRegime::Downtrend)
    {
        dip.push((68.0, "OBV rising (accumulation) despite weak price".into()));
    }

    // ── Distance below the 20-day average ──
    if let Some(sma20) = technical::sma(closes, 20) {
        let deviation = ((price - sma20) / sma20) * 100.0;
        if deviation < -3.0 && regime != MarketRegime::StrongDowntrend {
            dip.push((
                62.0 + deviation.abs().min(5.0) * 2.0,
                format!("Price {:.1}% below 20-SMA", deviation),
            ));
        }
    }

    if let Some(v) = family_vote("Dip", Direction::Long, dip) {
        strategies.push(v);
    }
    if let Some(v) = family_vote("Overextended", Direction::Short, stretched) {
        strategies.push(v);
    }

    let (direction, score, net_count) = composite(&strategies, session.volume_pace, regime);

    let confidence = match (score, net_count) {
        (s, c) if s >= 75.0 && c >= 3 => Confidence::High,
        (s, c) if s >= 60.0 && c >= 2 => Confidence::Medium,
        _ => Confidence::Low,
    };

    let atr = technical::atr(highs, lows, closes, 14);
    let atr_pct = atr.map(|a| (a / price) * 100.0);
    let change_pct = quote.regular_market_change_percent.unwrap_or(0.0);

    IntradaySignal {
        symbol: symbol.to_string(),
        name: quote
            .short_name
            .as_deref()
            .or(quote.long_name.as_deref())
            .unwrap_or("Unknown")
            .to_string(),
        price,
        currency: quote.currency.clone(),
        change_pct,
        volume_ratio,
        volume_known,
        regime,
        confidence,
        direction,
        score,
        strategies,
        rsi,
        vwap,
        above_vwap,
        macd_histogram,
        bb_position,
        atr_pct,
        obv_trend,
        gap_detected,
        near_support,
        near_resistance,
        pivot: pivot_val,
        support1: s1_val,
        resistance1: r1_val,
    }
}

/// One vote for a family of readings that describe the same move: as strong
/// as its strongest reading (at most 100), with every reason listed.
fn family_vote(
    name: &'static str,
    direction: Direction,
    readings: Vec<(f64, String)>,
) -> Option<StrategySignal> {
    let strength = readings.iter().map(|(s, _)| *s).reduce(f64::max)?;
    Some(StrategySignal {
        name,
        direction,
        strength: strength.min(100.0),
        reason: readings
            .into_iter()
            .map(|(_, r)| r)
            .collect::<Vec<_>>()
            .join("; "),
    })
}

/// Direction, score (0–100) and net vote count of a set of votes. Opposing
/// votes are netted, not dropped: the score is the net strength per
/// agreeing vote, the consensus bonus counts agreeing minus opposing votes,
/// and so does the count that sets the confidence. Then volume pace (unknown
/// counts as weak) and the regime scale it: a trade against any downtrend or
/// uptrend is penalised, more so against a strong one.
fn composite(
    strategies: &[StrategySignal],
    volume_pace: Option<f64>,
    regime: MarketRegime,
) -> (Direction, f64, usize) {
    let sum = |d: Direction| -> (f64, usize) {
        strategies
            .iter()
            .filter(|s| s.direction == d)
            .fold((0.0, 0), |(s, n), x| (s + x.strength, n + 1))
    };
    let (long_sum, long_n) = sum(Direction::Long);
    let (short_sum, short_n) = sum(Direction::Short);
    let (direction, agree, oppose) = if long_sum > short_sum {
        (Direction::Long, long_n, short_n)
    } else if short_sum > long_sum {
        (Direction::Short, short_n, long_n)
    } else {
        return (Direction::Long, 0.0, 0);
    };
    let net_strength = (long_sum - short_sum).abs();
    let net_count = agree.saturating_sub(oppose);
    let avg_strength = net_strength / agree as f64;
    let consensus_bonus = (net_count as f64 - 1.0).max(0.0) * 8.0;

    let vol_mult = match volume_pace {
        None => UNKNOWN_VOLUME_MULT,
        Some(r) if r > 1.5 => 1.15,
        Some(r) if r > 1.0 => 1.0,
        Some(_) => 0.85,
    };
    let regime_mult = match (direction, regime) {
        (Direction::Long, MarketRegime::StrongUptrend) => 1.2,
        (Direction::Long, MarketRegime::Uptrend) => 1.1,
        (Direction::Short, MarketRegime::StrongDowntrend) => 1.2,
        (Direction::Short, MarketRegime::Downtrend) => 1.1,
        // Counter-trend: penalised in any trend against the trade.
        (Direction::Long, MarketRegime::StrongDowntrend) => 0.6,
        (Direction::Long, MarketRegime::Downtrend) => 0.75,
        (Direction::Short, MarketRegime::StrongUptrend) => 0.6,
        (Direction::Short, MarketRegime::Uptrend) => 0.75,
        (_, MarketRegime::Ranging) => 1.0,
    };
    let score = ((avg_strength + consensus_bonus) * vol_mult * regime_mult).clamp(0.0, 100.0);
    (direction, score, net_count)
}

// ══════════════════════════════════════════════════════════
// POSITION SIZING
// ══════════════════════════════════════════════════════════

/// Half-Kelly fraction of capital to risk, from a measured win rate (0–1)
/// and average win over average loss. Capped at 25%.
fn kelly_fraction(win_rate: f64, win_loss_ratio: f64) -> f64 {
    let kelly = win_rate - (1.0 - win_rate) / win_loss_ratio;
    // Half-Kelly for safety
    (kelly * 0.5).clamp(0.0, 0.25)
}

/// Capital risked per trade (if its exit level is hit) until the paper
/// record has [`crate::simulator::MIN_TRADES_FOR_KELLY`] closed trades.
pub const FIXED_RISK_PCT: f64 = 0.5;

/// Risk per trade when the measured record shows no edge (Kelly ≤ 0): small,
/// so the paper record keeps growing and can show whether that changes.
pub const NO_EDGE_RISK_PCT: f64 = 0.25;

/// Sizing limits for [`generate_trade_plans_with`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlanLimits {
    /// Most % of capital to lose on one trade if its exit level is hit.
    pub max_risk_pct: f64,
    /// Max % of capital in one position.
    pub max_position_pct: f64,
    /// Max number of plans returned.
    pub max_positions: usize,
    /// Measured paper record (30+ closed trades) for Kelly sizing; None
    /// risks [`FIXED_RISK_PCT`] per trade.
    pub track_record: Option<crate::simulator::TrackRecord>,
    /// Paper charges and slippage counted inside the risk budget: a fill at
    /// the exact exit level loses no more than the budget, costs included.
    /// None sizes on price risk alone.
    pub costs: Option<crate::backtest::costs::CostModel>,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_risk_pct: 1.0,
            max_position_pct: 30.0,
            max_positions: 5,
            track_record: None,
            costs: None,
        }
    }
}

/// Whole shares in `x`, forgiving binary rounding: 99.99999999999 shares of
/// budget is 100 shares, not 99.
fn whole_shares(x: f64) -> u32 {
    if !(x.is_finite() && x > 0.0) {
        return 0;
    }
    (x + x.max(1.0) * 1e-9).floor().min(u32::MAX as f64) as u32
}

impl PlanLimits {
    /// Percent of capital risked per trade and the Kelly fraction behind it
    /// (0 without a track record).
    pub fn risk_pct(&self) -> (f64, f64) {
        match self.track_record {
            None => (FIXED_RISK_PCT.min(self.max_risk_pct), 0.0),
            Some(tr) => {
                let kf = kelly_fraction(tr.win_rate, tr.win_loss_ratio);
                let pct = if kf > 0.0 {
                    kf * 100.0
                } else {
                    NO_EDGE_RISK_PCT
                };
                (pct.min(self.max_risk_pct), kf)
            }
        }
    }
}

/// Generate trade plans with the default [`PlanLimits`] (30% per position,
/// 5 positions, the fixed 0.5% risk) and at most `max_risk_pct` risk per
/// trade.
pub fn generate_trade_plans(
    signals: &[IntradaySignal],
    capital: f64,
    max_risk_pct: f64,
) -> Vec<TradePlan> {
    generate_trade_plans_with(
        signals,
        capital,
        PlanLimits {
            max_risk_pct,
            ..PlanLimits::default()
        },
    )
}

/// Generate trade plans, highest score first, sized so that together they
/// never need more than `capital`.
///
/// - The exit level is at least 1 × ATR from the price: the prior session's
///   S1 (R1 for a short) is used only when it lies 1–2 ATR away.
/// - Each trade is sized so a fill at its exit level loses at most
///   [`PlanLimits::risk_pct`] of capital (with `costs`, charges included);
///   no position takes more than `max_position_pct`, which can leave the
///   loss below that budget.
/// - Walking down the ranking, a plan that doesn't fit in the capital left is
///   shrunk to what does (or dropped).
///
/// Ranking is by score alone: the reward-to-risk ratio of a level set this
/// way says nothing about the odds, so it no longer lifts a plan.
pub fn generate_trade_plans_with(
    signals: &[IntradaySignal],
    capital: f64,
    limits: PlanLimits,
) -> Vec<TradePlan> {
    let (risk_pct, kf) = limits.risk_pct();
    let max_risk_per_trade = capital * (risk_pct / 100.0);
    let mut plans = Vec::new();

    for signal in signals
        .iter()
        .filter(|s| s.score >= 50.0 && s.confidence != Confidence::Low)
    {
        let price = signal.price;
        // No ATR, no plan: the levels and the size both depend on it.
        let Some(atr_pct) = signal.atr_pct.filter(|a| a.is_finite()) else {
            continue;
        };
        if atr_pct < 0.3 {
            continue; // not enough daily range
        }
        let atr = price * atr_pct / 100.0;

        // Dynamic target based on regime + score
        let target_multiplier = match signal.regime {
            MarketRegime::StrongUptrend if signal.direction == Direction::Long => 0.7,
            MarketRegime::Uptrend if signal.direction == Direction::Long => 0.55,
            MarketRegime::Ranging => 0.4,
            _ => 0.45,
        };
        let move_pct = atr_pct * target_multiplier;

        // Two targets: first at 60% of range, second at full range
        let target1 = match signal.direction {
            Direction::Long => price * (1.0 + move_pct * 0.6 / 100.0),
            Direction::Short => price * (1.0 - move_pct * 0.6 / 100.0),
        };
        let target2 = match signal.direction {
            Direction::Long => price * (1.0 + move_pct / 100.0),
            Direction::Short => price * (1.0 - move_pct / 100.0),
        };

        let stop_loss = exit_level(
            signal.direction,
            price,
            atr,
            signal.support1,
            signal.resistance1,
        );

        // Trailing exit level: 1.5 × ATR from the price.
        let trailing_stop = match signal.direction {
            Direction::Long => price - 1.5 * atr,
            Direction::Short => price + 1.5 * atr,
        };
        // A level at or below zero (a huge ATR) could never be reached by a
        // live price: no plan.
        if ![stop_loss, trailing_stop, target1, target2]
            .iter()
            .all(|l| l.is_finite() && *l > 0.0)
        {
            continue;
        }

        let risk_per_share = (price - stop_loss).abs();
        let profit_per_share = (target2 - price).abs();
        if risk_per_share < f64::EPSILON {
            continue;
        }
        let risk_reward = profit_per_share / risk_per_share;

        // Size from the risk budget, capped per position. The loss at the
        // exact exit level includes the round trip's charges and slippage.
        let long = signal.direction == Direction::Long;
        let loss_at_exit = |qty: u32| {
            qty as f64 * risk_per_share
                + limits.costs.as_ref().map_or(0.0, |m| {
                    crate::simulator::round_trip_cost(m, long, qty, price, &[(qty, stop_loss)])
                })
        };
        let max_capital = capital * (limits.max_position_pct / 100.0);
        let cap_qty = whole_shares(max_capital / price);
        let mut qty = whole_shares(max_risk_per_trade / risk_per_share).min(cap_qty);
        if qty > 0 && limits.costs.is_some() {
            // One proportional step for the costs, then share by share.
            let per_share = loss_at_exit(qty) / qty as f64;
            qty = qty.min(whole_shares(max_risk_per_trade / per_share));
            let budget = max_risk_per_trade * (1.0 + 1e-9);
            while qty > 0 && loss_at_exit(qty) > budget {
                qty -= 1;
            }
        }
        if qty == 0 {
            continue;
        }

        let capital_required = qty as f64 * price;
        let max_risk = loss_at_exit(qty);
        let expected_profit = qty as f64 * profit_per_share;
        let position_pct = (capital_required / capital) * 100.0;

        plans.push(TradePlan {
            signal: signal.clone_signal(),
            entry: price,
            target1,
            target2,
            stop_loss,
            trailing_stop,
            qty,
            capital_required,
            max_risk,
            expected_profit,
            risk_reward,
            kelly_fraction: kf,
            position_pct,
        });
    }

    // Highest score first (stable: ties keep the scan's order).
    plans.sort_by(|a, b| b.signal.score.total_cmp(&a.signal.score));

    cap_total_allocation(plans, capital, limits.max_positions, limits.costs.as_ref())
}

/// The exit level for a paper trade: 1 × ATR from the price, or the prior
/// session's S1 (R1 for a short) when it is 1–2 ATR away. A level closer than
/// 1 ATR would be hit by ordinary noise; one on the wrong side of the price,
/// or past 2 ATR, is ignored.
fn exit_level(
    direction: Direction,
    price: f64,
    atr: f64,
    support1: Option<f64>,
    resistance1: Option<f64>,
) -> f64 {
    match direction {
        Direction::Long => support1
            .filter(|s| (price - 2.0 * atr..=price - atr).contains(s))
            .unwrap_or(price - atr),
        Direction::Short => resistance1
            .filter(|r| (price + atr..=price + 2.0 * atr).contains(r))
            .unwrap_or(price + atr),
    }
}

/// Keep the best `max_positions` plans whose combined capital fits in
/// `capital`, shrinking the plan that would overflow and dropping any that
/// can't afford a single share.
fn cap_total_allocation(
    plans: Vec<TradePlan>,
    capital: f64,
    max_positions: usize,
    costs: Option<&crate::backtest::costs::CostModel>,
) -> Vec<TradePlan> {
    let mut remaining = capital;
    let mut kept = Vec::new();
    for mut plan in plans {
        if kept.len() >= max_positions {
            break;
        }
        if plan.capital_required > remaining {
            let qty = whole_shares(remaining / plan.entry);
            if qty == 0 {
                continue;
            }
            let per_share_profit = plan.expected_profit / plan.qty as f64;
            plan.qty = qty;
            plan.capital_required = qty as f64 * plan.entry;
            // Recomputed, not scaled: per-order charges don't shrink with
            // the quantity.
            let long = plan.signal.direction == Direction::Long;
            plan.max_risk = qty as f64 * (plan.entry - plan.stop_loss).abs()
                + costs.map_or(0.0, |m| {
                    crate::simulator::round_trip_cost(
                        m,
                        long,
                        qty,
                        plan.entry,
                        &[(qty, plan.stop_loss)],
                    )
                });
            plan.expected_profit = qty as f64 * per_share_profit;
            plan.position_pct = plan.capital_required / capital * 100.0;
        }
        remaining -= plan.capital_required;
        kept.push(plan);
    }
    kept
}

// ══════════════════════════════════════════════════════════
// SECTOR HEAT
// ══════════════════════════════════════════════════════════

pub async fn scan_sector_heat(client: &YahooClient) -> Result<Vec<SectorHeat>> {
    let sectors = market::INDIA_SECTOR_INDICES;
    let syms: Vec<&str> = sectors.iter().map(|(s, _)| *s).collect();
    let quotes = client.get_quote(&syms).await?;

    let mut heats: Vec<SectorHeat> = quotes
        .iter()
        .enumerate()
        .filter_map(|(i, q)| {
            let name = sectors.get(i).map(|(_, n)| n.to_string())?;
            let pct = q.regular_market_change_percent.unwrap_or(0.0);
            Some(SectorHeat {
                name,
                change_pct: pct,
                hot: pct > 0.5,
            })
        })
        .collect();

    heats.sort_by(|a, b| {
        b.change_pct
            .partial_cmp(&a.change_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(heats)
}

// ══════════════════════════════════════════════════════════
// SCANNER
// ══════════════════════════════════════════════════════════

/// Most symbols an intraday scan fetches bars for: the most traded by
/// value of the market's universe.
const INTRADAY_SCAN_LIMIT: usize = 40;

/// Daily history the scan reads: about 63 bars, past MACD's 35.
const DAILY_RANGE: &str = "3mo";

pub async fn scan_intraday(
    client: &YahooClient,
    market: market::Market,
) -> Result<Vec<IntradaySignal>> {
    scan_intraday_with(client, market, None).await
}

/// A strategy file's rules (see `backtest -s`) as one more vote: a Long
/// "Custom rule" signal when its entry rule holds on the latest daily bar, a
/// Short one when its exit rule holds. Uses its own daily history, long
/// enough for the rule's indicators, so the built-in signals are unchanged.
/// No history means no vote; a failing WASM plugin is an error.
async fn custom_rule_signal(
    client: &YahooClient,
    symbol: &str,
    rule: &crate::backtest::rules::Strategy,
) -> Result<Option<StrategySignal>> {
    let range = match rule.lookback() {
        0..200 => "1y",
        200..450 => "2y",
        _ => "5y",
    };
    let Ok(chart) = client.get_chart(symbol, range, "1d").await else {
        return Ok(None);
    };
    rule_vote(rule, &crate::backtest::Bars::from_chart(&chart))
        .with_context(|| format!("strategy signal for {symbol}"))
}

fn rule_vote(
    rule: &crate::backtest::rules::Strategy,
    bars: &crate::backtest::Bars,
) -> Result<Option<StrategySignal>> {
    if bars.len() <= rule.lookback() {
        return Ok(None);
    }
    let signals = rule.signals(bars)?;
    let last = bars.len() - 1;
    Ok(if signals.entry[last] && !signals.exit[last] {
        Some(StrategySignal {
            name: "Custom rule",
            direction: Direction::Long,
            strength: 70.0,
            reason: format!("{}: entry rule holds", rule.name),
        })
    } else if signals.exit[last] {
        Some(StrategySignal {
            name: "Custom rule",
            direction: Direction::Short,
            strength: 60.0,
            reason: format!("{}: exit rule holds", rule.name),
        })
    } else {
        None
    })
}

/// [`scan_intraday`] with an optional strategy rule as an extra signal.
pub async fn scan_intraday_with(
    client: &YahooClient,
    market: market::Market,
    rule: Option<&crate::backtest::rules::Strategy>,
) -> Result<Vec<IntradaySignal>> {
    let symbols = market.universe();
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let mut quotes = client.get_quote(&sym_refs).await?;
    // The scan is for liquid names and each one costs a chart request
    // (spaced 200 ms apart): keep the most traded by value today, so a
    // whole-index universe still scans in seconds and entries are not
    // planned on stale prices.
    if quotes.len() > INTRADAY_SCAN_LIMIT {
        let traded = |q: &crate::api::Quote| {
            q.regular_market_price.unwrap_or(0.0) * q.regular_market_volume.unwrap_or(0) as f64
        };
        quotes.sort_by(|a, b| traded(b).total_cmp(&traded(a)));
        quotes.truncate(INTRADAY_SCAN_LIMIT);
    }
    let mut signals = Vec::new();

    // Daily bars (three months, enough for MACD's 35) and the last five
    // sessions of 5-minute bars (today's VWAP and volume pace), a few
    // symbols at a time, in the quotes' order; priceless quotes skip both.
    use futures_util::StreamExt;
    type Charts = (Option<crate::api::ChartData>, Option<crate::api::ChartData>);
    let charts: Vec<Charts> = futures_util::stream::iter(quotes.iter().map(|q| async move {
        let Some(sym) = q.symbol.as_deref() else {
            return (None, None);
        };
        if q.regular_market_price.is_none_or(|p| p <= 0.0) {
            return (None, None);
        }
        let (daily, intraday) = futures_util::join!(
            client.get_chart(sym, DAILY_RANGE, "1d"),
            client.get_chart(sym, "5d", "5m"),
        );
        (daily.ok(), intraday.ok())
    }))
    .buffered(8)
    .collect()
    .await;

    let now = chrono::Utc::now();
    for (quote, (daily, intraday)) in quotes.iter().zip(charts) {
        let symbol = match &quote.symbol {
            Some(s) => s.clone(),
            None => continue,
        };
        let price = match quote.regular_market_price {
            Some(p) if p > 0.0 => p,
            _ => continue,
        };
        let Some(chart) = daily else {
            continue;
        };
        // Columns aligned by index: a bar with any missing price is dropped
        // whole, never shifted against the others.
        let bars = crate::backtest::Bars::from_chart(&chart);
        if bars.len() < 15 {
            continue;
        }
        let sym_market = market::market_of_symbol(&symbol, market);
        let session = intraday
            .map(|c| {
                session_stats(
                    &intraday_bars(&c),
                    sym_market,
                    crate::clock::session_date(sym_market, now),
                )
            })
            .unwrap_or_default();

        let custom = match rule {
            Some(rule) => custom_rule_signal(client, &symbol, rule).await?,
            None => None,
        };
        let prior = chart.prior_session(now, sym_market);
        let signal = compute_signal(&symbol, quote, price, &bars, session, prior, custom);
        signals.push(signal);
    }

    signals.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(signals)
}

// Helper to clone signal without implementing Clone on the whole struct
impl IntradaySignal {
    pub fn clone_signal(&self) -> IntradaySignal {
        IntradaySignal {
            symbol: self.symbol.clone(),
            name: self.name.clone(),
            price: self.price,
            currency: self.currency.clone(),
            change_pct: self.change_pct,
            volume_ratio: self.volume_ratio,
            volume_known: self.volume_known,
            regime: self.regime,
            confidence: self.confidence,
            direction: self.direction,
            score: self.score,
            strategies: self.strategies.clone(),
            rsi: self.rsi,
            vwap: self.vwap,
            above_vwap: self.above_vwap,
            macd_histogram: self.macd_histogram,
            bb_position: self.bb_position,
            atr_pct: self.atr_pct,
            obv_trend: self.obv_trend,
            gap_detected: self.gap_detected,
            near_support: self.near_support,
            near_resistance: self.near_resistance,
            pivot: self.pivot,
            support1: self.support1,
            resistance1: self.resistance1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vwap_reclaim_needs_a_cross() {
        // Crossed up from below: a reclaim.
        assert!(matches!(
            vwap_cross(99.8, 100.2, 100.0),
            Some((Direction::Long, _))
        ));
        // Already above on the previous bar: no new reclaim.
        assert!(vwap_cross(100.3, 100.2, 100.0).is_none());
        // Crossed down from above: a rejection.
        assert!(matches!(
            vwap_cross(100.1, 99.8, 100.0),
            Some((Direction::Short, _))
        ));
        // Already below: nothing.
        assert!(vwap_cross(99.5, 99.8, 100.0).is_none());
        // Too far past the VWAP to call it a reclaim.
        assert!(vwap_cross(99.0, 101.0, 100.0).is_none());
    }

    #[test]
    fn obv_trend_reads_negative_series_correctly() {
        // OBV is a signed running total, so a stock with historical selling
        // sits at a negative level. The band must scale with |avg|: a 2%
        // drift is noise, not accumulation or distribution.
        let series = |last: f64| {
            let mut v = vec![-1_000_000.0; 11];
            v[10] = last;
            v
        };
        assert_eq!(obv_trend(&series(-1_020_000.0)), OBVTrend::Flat);
        assert_eq!(obv_trend(&series(-980_000.0)), OBVTrend::Flat);
        assert_eq!(obv_trend(&series(-1_200_000.0)), OBVTrend::Falling);
        assert_eq!(obv_trend(&series(-800_000.0)), OBVTrend::Rising);
        // Positive series keep working, and a flat series stays neutral.
        let positive: Vec<f64> = (0..12).map(|i| 50_000.0 + i as f64 * 2_000.0).collect();
        assert_eq!(obv_trend(&positive), OBVTrend::Rising);
        assert_eq!(obv_trend(&[-1_000_000.0; 12]), OBVTrend::Flat);
        assert_eq!(obv_trend(&[-1.0; 5]), OBVTrend::Flat);
    }

    fn make_signal(score: f64, confidence: Confidence, atr_pct: f64) -> IntradaySignal {
        IntradaySignal {
            symbol: "TEST.NS".into(),
            name: "Test Corp".into(),
            price: 100.0,
            currency: None,
            change_pct: 0.5,
            volume_ratio: 1.2,
            volume_known: true,
            regime: MarketRegime::Uptrend,
            confidence,
            direction: Direction::Long,
            score,
            strategies: vec![],
            rsi: Some(50.0),
            vwap: None,
            above_vwap: false,
            macd_histogram: None,
            bb_position: None,
            atr_pct: Some(atr_pct),
            obv_trend: OBVTrend::Flat,
            gap_detected: false,
            near_support: false,
            near_resistance: false,
            pivot: None,
            support1: None,
            resistance1: None,
        }
    }

    #[test]
    fn test_direction_display() {
        assert_eq!(format!("{}", Direction::Long), "long");
        assert_eq!(format!("{}", Direction::Short), "short");
        // Stored strings from before 0.8.1 still read the same way.
        for (s, long) in [
            ("BUY", true),
            ("SELL", false),
            ("long", true),
            ("short", false),
        ] {
            assert_eq!(Direction::is_long_str(s), long, "{s}");
        }
        assert_eq!(Direction::label_of("BUY"), "long");
        assert_eq!(Direction::label_of("SELL"), "short");
    }

    #[test]
    fn test_market_regime_display() {
        assert_eq!(format!("{}", MarketRegime::StrongUptrend), "Strong Uptrend");
        assert_eq!(format!("{}", MarketRegime::Uptrend), "Uptrend");
        assert_eq!(format!("{}", MarketRegime::Ranging), "Ranging");
        assert_eq!(format!("{}", MarketRegime::Downtrend), "Downtrend");
        assert_eq!(
            format!("{}", MarketRegime::StrongDowntrend),
            "Strong Downtrend"
        );
    }

    #[test]
    fn test_confidence_display() {
        assert_eq!(format!("{}", Confidence::High), "HIGH");
        assert_eq!(format!("{}", Confidence::Medium), "MEDIUM");
        assert_eq!(format!("{}", Confidence::Low), "LOW");
    }

    #[test]
    fn test_obv_trend_display() {
        assert_eq!(format!("{}", OBVTrend::Rising), "Accumulation");
        assert_eq!(format!("{}", OBVTrend::Falling), "Distribution");
        assert_eq!(format!("{}", OBVTrend::Flat), "Neutral");
    }

    #[test]
    fn test_kelly_fraction_positive() {
        let kf = kelly_fraction(0.6, 2.0);
        assert!(kf > 0.0);
        assert!(kf <= 0.25);
    }

    #[test]
    fn test_kelly_fraction_clamped_at_max() {
        // Even perfect win rate capped at 0.25
        let kf = kelly_fraction(1.0, 10.0);
        assert_eq!(kf, 0.25);
    }

    #[test]
    fn test_kelly_fraction_clamped_at_zero() {
        // Bad edge: 30% win rate, 1:1 R:R → kelly is negative → clamp to 0
        let kf = kelly_fraction(0.3, 1.0);
        assert_eq!(kf, 0.0);
    }

    #[test]
    fn test_kelly_fraction_half_applied() {
        // kelly(0.6, 2.0) = 0.6 - 0.4/2 = 0.4 → half = 0.2
        let kf = kelly_fraction(0.6, 2.0);
        assert!((kf - 0.2).abs() < 1e-9);
    }

    #[test]
    fn test_detect_regime_insufficient_data() {
        let closes = vec![100.0; 10];
        assert_eq!(detect_regime(&closes), MarketRegime::Ranging);
    }

    #[test]
    fn test_detect_regime_uptrend() {
        // Steadily rising prices — price > sma10 > sma20
        let closes: Vec<f64> = (0..30).map(|i| 100.0 + i as f64 * 2.0).collect();
        let regime = detect_regime(&closes);
        assert!(regime == MarketRegime::StrongUptrend || regime == MarketRegime::Uptrend);
    }

    #[test]
    fn test_detect_regime_downtrend() {
        // Steadily falling prices
        let closes: Vec<f64> = (0..30).map(|i| 200.0 - i as f64 * 3.0).collect();
        let regime = detect_regime(&closes);
        assert!(regime == MarketRegime::Downtrend || regime == MarketRegime::StrongDowntrend);
    }

    #[test]
    fn test_detect_regime_ranging() {
        // Flat/sideways prices
        let closes = vec![100.0; 30];
        let regime = detect_regime(&closes);
        assert_eq!(regime, MarketRegime::Ranging);
    }

    #[test]
    fn test_generate_trade_plans_empty() {
        let plans = generate_trade_plans(&[], 25000.0, 1.0);
        assert!(plans.is_empty());
    }

    #[test]
    fn test_generate_trade_plans_low_score_filtered() {
        let signal = make_signal(30.0, Confidence::Low, 2.0); // score below 50
        let plans = generate_trade_plans(&[signal], 25000.0, 1.0);
        assert!(plans.is_empty());
    }

    #[test]
    fn test_generate_trade_plans_low_confidence_filtered() {
        let signal = make_signal(80.0, Confidence::Low, 2.0); // Low confidence filtered
        let plans = generate_trade_plans(&[signal], 25000.0, 1.0);
        assert!(plans.is_empty());
    }

    #[test]
    fn test_generate_trade_plans_low_atr_filtered() {
        let signal = make_signal(80.0, Confidence::High, 0.2); // ATR too small
        let plans = generate_trade_plans(&[signal], 25000.0, 1.0);
        assert!(plans.is_empty());
    }

    #[test]
    fn test_generate_trade_plans_valid_signal() {
        let signal = make_signal(75.0, Confidence::High, 2.5);
        let plans = generate_trade_plans(&[signal], 25000.0, 1.0);
        // Should generate a plan
        assert!(!plans.is_empty());
        let plan = &plans[0];
        assert!(plan.qty > 0);
        assert!(plan.target1 > plan.entry);
        assert!(plan.stop_loss < plan.entry);
        assert!(plan.risk_reward > 0.0);
    }

    #[test]
    fn plans_are_ranked_by_score_alone() {
        let s1 = make_signal(60.0, Confidence::Medium, 2.5);
        let mut s2 = make_signal(85.0, Confidence::High, 3.0);
        s2.symbol = "BETTER.NS".into();
        let plans = generate_trade_plans(&[s1, s2], 50000.0, 1.0);
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].signal.symbol, "BETTER.NS");
    }

    #[test]
    fn exit_level_is_never_tighter_than_one_atr() {
        // ATR 2% of 100 = 2. An S1 just under the price (a "Support Bounce"
        // fires within 0.5% of it) used to become a 0.1-point stop.
        for s1 in [99.9, 99.0, 98.5, 101.0] {
            let mut long = make_signal(80.0, Confidence::High, 2.0);
            long.support1 = Some(s1);
            let plan = &generate_trade_plans(&[long], 25000.0, 1.0)[0];
            assert!(
                plan.entry - plan.stop_loss >= 2.0 - 1e-9,
                "S1 {s1}: {}",
                plan.stop_loss
            );
        }
        // An S1 1–2 ATR away is used as it is.
        let mut long = make_signal(80.0, Confidence::High, 2.0);
        long.support1 = Some(97.0);
        assert_eq!(
            generate_trade_plans(&[long], 25000.0, 1.0)[0].stop_loss,
            97.0
        );
        // Too far away: back to 1 ATR.
        let mut long = make_signal(80.0, Confidence::High, 2.0);
        long.support1 = Some(90.0);
        assert!((generate_trade_plans(&[long], 25000.0, 1.0)[0].stop_loss - 98.0).abs() < 1e-9);
        // Shorts mirror it with R1.
        let mut short = make_signal(80.0, Confidence::High, 2.0);
        short.direction = Direction::Short;
        short.resistance1 = Some(100.1);
        let plan = &generate_trade_plans(&[short], 25000.0, 1.0)[0];
        assert!(plan.stop_loss - plan.entry >= 2.0 - 1e-9);
        // The trail hangs 1.5 ATR behind.
        assert!((plan.trailing_stop - 103.0).abs() < 1e-9);
    }

    #[test]
    fn a_tight_support_no_longer_buys_rank_or_size() {
        // Same score; one sits right on S1. Before, its 0.1-point stop gave an
        // R:R near 9, the largest Kelly size and the top rank.
        let mut tight = make_signal(70.0, Confidence::High, 2.0);
        tight.symbol = "TIGHT.NS".into();
        tight.support1 = Some(99.9);
        let mut plain = make_signal(75.0, Confidence::High, 2.0);
        plain.symbol = "PLAIN.NS".into();
        let plans = generate_trade_plans(&[tight, plain], 1_000_000.0, 1.0);
        assert_eq!(plans[0].signal.symbol, "PLAIN.NS");
        let tight = plans
            .iter()
            .find(|p| p.signal.symbol == "TIGHT.NS")
            .unwrap();
        assert!(tight.risk_reward < 1.0, "R:R {}", tight.risk_reward);
        // Both risk the same fixed 0.5% of capital: ₹5,000 over a 2-point stop.
        for p in &plans {
            assert!(p.max_risk <= 5_000.0 + 1e-6, "{}", p.max_risk);
            assert!(p.max_risk >= 5_000.0 - p.entry, "{}", p.max_risk);
            assert_eq!(p.kelly_fraction, 0.0);
        }
    }

    #[test]
    fn kelly_needs_a_measured_record() {
        let limits = PlanLimits::default();
        assert_eq!(limits.risk_pct(), (FIXED_RISK_PCT, 0.0));
        // 55% won, wins 1.5× losses: half-Kelly = (0.55 − 0.45/1.5) / 2 = 0.125,
        // capped by the 1% risk limit.
        let tr = crate::simulator::TrackRecord {
            trades: 40,
            win_rate: 0.55,
            win_loss_ratio: 1.5,
        };
        let with = PlanLimits {
            track_record: Some(tr),
            ..limits.clone()
        };
        let (pct, kf) = with.risk_pct();
        assert!((kf - 0.125).abs() < 1e-9);
        assert_eq!(pct, 1.0);
        // No edge measured: the small floor, not zero (the record must grow).
        let none = PlanLimits {
            track_record: Some(crate::simulator::TrackRecord {
                win_rate: 0.3,
                win_loss_ratio: 1.0,
                ..tr
            }),
            ..limits
        };
        assert_eq!(none.risk_pct(), (NO_EDGE_RISK_PCT, 0.0));
    }

    fn quote() -> Quote {
        Quote::default()
    }

    fn strategy_names(sig: &IntradaySignal) -> Vec<&'static str> {
        sig.strategies.iter().map(|s| s.name).collect()
    }

    #[test]
    fn dip_readings_make_one_vote() {
        // Sixty calm days, then a slide: RSI low, under the lower band and
        // well below the 20-day average all at once.
        let mut closes: Vec<f64> = (0..60).map(|i| 100.0 + (i % 2) as f64 * 0.5).collect();
        for i in 0..8 {
            closes.push(99.0 - i as f64 * 1.5);
        }
        let bars = crate::backtest::Bars::from_closes(&closes);
        let price = *closes.last().unwrap();
        let sig = compute_signal(
            "T.NS",
            &quote(),
            price,
            &bars,
            SessionStats::default(),
            None,
            None,
        );
        let names = strategy_names(&sig);
        assert_eq!(
            names.iter().filter(|n| **n == "Dip").count(),
            1,
            "{names:?}"
        );
        for old in [
            "RSI Reversal",
            "BB Bounce",
            "Mean Reversion",
            "OBV Divergence",
        ] {
            assert!(!names.contains(&old), "{old} in {names:?}");
        }
        let dip = sig.strategies.iter().find(|s| s.name == "Dip").unwrap();
        assert!(dip.strength <= 100.0);
        // Several readings fired; they share the one vote.
        assert!(
            dip.reason.contains("RSI") && dip.reason.contains("Bollinger"),
            "{}",
            dip.reason
        );
    }

    #[test]
    fn rsi_strength_is_clamped_and_families_merge() {
        let vote = family_vote(
            "Dip",
            Direction::Long,
            vec![(85.0 + 30.0, "RSI 0".into()), (65.0, "band".into())],
        )
        .unwrap();
        assert_eq!(vote.strength, 100.0);
        assert_eq!(vote.reason, "RSI 0; band");
        assert!(family_vote("Dip", Direction::Long, vec![]).is_none());
    }

    fn vote(direction: Direction, strength: f64) -> StrategySignal {
        StrategySignal {
            name: "x",
            direction,
            strength,
            reason: String::new(),
        }
    }

    #[test]
    fn opposing_votes_are_netted() {
        let longs = vec![vote(Direction::Long, 70.0); 3];
        let (d, alone, n) = composite(&longs, Some(1.2), MarketRegime::Ranging);
        assert_eq!((d, n), (Direction::Long, 3));
        assert!((alone - 86.0).abs() < 1e-9); // 70 + 2 × 8
        let mut mixed = longs.clone();
        mixed.push(vote(Direction::Short, 60.0));
        let (d, netted, n) = composite(&mixed, Some(1.2), MarketRegime::Ranging);
        assert_eq!((d, n), (Direction::Long, 2));
        // (210 − 60) / 3 = 50, + 1 × 8.
        assert!((netted - 58.0).abs() < 1e-9, "{netted}");
        // Unknown volume is penalised like a quiet day.
        let (_, unknown, _) = composite(&longs, None, MarketRegime::Ranging);
        assert!((unknown - 86.0 * UNKNOWN_VOLUME_MULT).abs() < 1e-9);
    }

    #[test]
    fn counter_trend_penalty_applies_in_any_downtrend() {
        let longs = vec![vote(Direction::Long, 70.0)];
        let score = |r| composite(&longs, Some(1.2), r).1;
        assert!(score(MarketRegime::Downtrend) < score(MarketRegime::Ranging));
        assert!(score(MarketRegime::StrongDowntrend) < score(MarketRegime::Downtrend));
        let shorts = vec![vote(Direction::Short, 70.0)];
        let score = |r| composite(&shorts, Some(1.2), r).1;
        assert!(score(MarketRegime::Uptrend) < score(MarketRegime::Ranging));
    }

    #[test]
    fn macd_fires_with_three_months_of_daily_bars() {
        // A slide then a recovery: find the bar where the MACD histogram
        // turns up and stop the series there.
        let series: Vec<f64> = (0..63)
            .map(|i| {
                let i = i as f64;
                if i < 40.0 {
                    120.0 - i * 0.8
                } else {
                    88.0 + (i - 40.0) * 1.5
                }
            })
            .collect();
        let crossed = (37..=series.len())
            .find(|&n| {
                let h = |k: usize| technical::macd(&series[..k]).map(|(_, _, h)| h);
                matches!((h(n), h(n - 1)), (Some(a), Some(b)) if a > 0.0 && b <= 0.0)
            })
            .expect("the recovery turns the histogram up within three months");
        let closes = &series[..crossed];
        let sig = compute_signal(
            "T.NS",
            &quote(),
            *closes.last().unwrap(),
            &crate::backtest::Bars::from_closes(closes),
            SessionStats::default(),
            None,
            None,
        );
        assert!(
            strategy_names(&sig).contains(&"MACD Cross"),
            "{:?}",
            strategy_names(&sig)
        );
        // The old one-month window (about 21 bars) can never see a cross.
        let short = &closes[closes.len() - 21..];
        let sig = compute_signal(
            "T.NS",
            &quote(),
            *short.last().unwrap(),
            &crate::backtest::Bars::from_closes(short),
            SessionStats::default(),
            None,
            None,
        );
        assert!(!strategy_names(&sig).contains(&"MACD Cross"));
    }

    fn chart(json: serde_json::Value) -> crate::api::ChartData {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_null_in_one_column_drops_the_whole_bar() {
        // Bar 1 has no close. Filtering each column separately used to
        // shift every close after it against its high, low and volume.
        let c = chart(serde_json::json!({
            "timestamp": [1_700_000_000i64, 1_700_086_400i64, 1_700_172_800i64],
            "indicators": {"quote": [{
                "open":   [10.0, 20.0, 30.0],
                "high":   [11.0, 21.0, 31.0],
                "low":    [9.0, 19.0, 29.0],
                "close":  [10.5, null, 30.5],
                "volume": [100, 200, 300]
            }]}
        }));
        let daily = crate::backtest::Bars::from_chart(&c);
        assert_eq!(daily.close, vec![10.5, 30.5]);
        assert_eq!(daily.high, vec![11.0, 31.0]);
        assert_eq!(daily.volume, vec![100, 300]);
        let intraday = intraday_bars(&c);
        assert_eq!(intraday.len(), 2);
        assert_eq!(
            (intraday[1].high, intraday[1].close, intraday[1].volume),
            (31.0, 30.5, Some(300))
        );
    }

    /// 5-minute bars for an NSE session dated `day` of October 2026, from
    /// 09:15 IST, one per volume given.
    fn session_bars(day: u32, price: f64, volumes: &[u64]) -> Vec<IntradayBar> {
        let date = NaiveDate::from_ymd_opt(2026, 10, day).unwrap();
        let open = crate::clock::at(
            market::Market::In,
            date,
            chrono::NaiveTime::from_hms_opt(9, 15, 0).unwrap(),
        )
        .timestamp();
        volumes
            .iter()
            .enumerate()
            .map(|(i, &v)| IntradayBar {
                time: open + i as i64 * 300,
                high: price + 1.0,
                low: price - 1.0,
                close: price,
                volume: Some(v),
            })
            .collect()
    }

    #[test]
    fn measured_zero_volume_counts_and_missing_volume_is_unknown() {
        // Earlier sessions traded 0 and 200 by this minute; today 300: 3.0.
        let mut bars = session_bars(26, 50.0, &[0, 0, 900]);
        bars.extend(session_bars(27, 50.0, &[100, 100, 900]));
        bars.extend(session_bars(28, 60.0, &[200, 100]));
        let today = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        let s = session_stats(&bars, market::Market::In, today);
        assert!((s.volume_pace.unwrap() - 3.0).abs() < 1e-9, "{s:?}");
        // Today's opening bar without volume: the pace is unknown, not 0;
        // the VWAP still uses the bars that have volume.
        let n = bars.len();
        bars[n - 2].volume = None;
        let s = session_stats(&bars, market::Market::In, today);
        assert_eq!(s.volume_pace, None);
        assert!((s.vwap.unwrap() - 60.0).abs() < 1e-9);
    }

    #[test]
    fn macd_votes_as_soon_as_two_histograms_exist() {
        // 35 flat closes, then one up: the 35-bar histogram is 0 and the
        // 36-bar one positive.
        let mut closes = vec![100.0; 35];
        closes.push(101.0);
        let sig = compute_signal(
            "T.NS",
            &quote(),
            101.0,
            &crate::backtest::Bars::from_closes(&closes),
            SessionStats::default(),
            None,
            None,
        );
        assert!(
            strategy_names(&sig).contains(&"MACD Cross"),
            "{:?}",
            strategy_names(&sig)
        );
    }

    #[test]
    fn levels_at_or_below_zero_get_no_plan() {
        // ATR of 110%: a 1-ATR exit level would be −10.
        let signal = make_signal(80.0, Confidence::High, 110.0);
        assert!(generate_trade_plans(&[signal], 25000.0, 1.0).is_empty());
    }

    #[test]
    fn share_counts_survive_binary_rounding() {
        // ₹240 budget (0.5% of ₹48,000) over a ₹2.40 stop is 100 shares;
        // 100 − 97.6 is a hair over 2.4 in binary.
        let mut signal = make_signal(80.0, Confidence::High, 2.0);
        signal.support1 = Some(97.6);
        let limits = PlanLimits {
            max_position_pct: 100.0,
            ..PlanLimits::default()
        };
        let plans = generate_trade_plans_with(&[signal], 48_000.0, limits);
        assert_eq!(plans[0].qty, 100);
    }

    #[test]
    fn a_shrunk_plan_recomputes_its_cost_inclusive_risk() {
        // Flat ₹1 per order: 100 shares at 100 with the exit level at 98
        // lose 200 + 2. Shrunk to 50 by the capital left they lose 100 + 2,
        // not half of 202.
        let mut flat = crate::backtest::costs::CostModel::none();
        flat.brokerage.flat = Some(1.0);
        let plan = TradePlan {
            signal: make_signal(80.0, Confidence::High, 2.0),
            entry: 100.0,
            target1: 101.0,
            target2: 102.0,
            stop_loss: 98.0,
            trailing_stop: 97.0,
            qty: 100,
            capital_required: 10_000.0,
            max_risk: 202.0,
            expected_profit: 200.0,
            risk_reward: 1.0,
            kelly_fraction: 0.0,
            position_pct: 0.0,
        };
        let kept = cap_total_allocation(vec![plan], 5_000.0, 5, Some(&flat));
        assert_eq!(kept[0].qty, 50);
        assert!(
            (kept[0].max_risk - 102.0).abs() < 1e-9,
            "{}",
            kept[0].max_risk
        );
    }

    #[test]
    fn the_risk_budget_includes_costs_at_the_exit_level() {
        // $25,000, entry 100, exit level 98: 62 shares lose $124 on price
        // alone but about $130 with slippage. With costs counted the loss at
        // the exit level stays within the $125 budget.
        let signal = make_signal(80.0, Confidence::High, 2.0);
        let costs = crate::backtest::costs::CostModel::us();
        let gross =
            generate_trade_plans_with(&[signal.clone_signal()], 25_000.0, PlanLimits::default());
        assert_eq!(gross[0].qty, 62);
        let limits = PlanLimits {
            costs: Some(costs.clone()),
            ..PlanLimits::default()
        };
        let net = &generate_trade_plans_with(&[signal], 25_000.0, limits)[0];
        let loss = net.qty as f64 * 2.0
            + crate::simulator::round_trip_cost(&costs, true, net.qty, 100.0, &[(net.qty, 98.0)]);
        assert!(loss <= 125.0 + 1e-9, "{loss}");
        assert!((net.max_risk - loss).abs() < 1e-9);
        // The largest such size: one more share would break the budget.
        let more = (net.qty + 1) as f64 * 2.0
            + crate::simulator::round_trip_cost(
                &costs,
                true,
                net.qty + 1,
                100.0,
                &[(net.qty + 1, 98.0)],
            );
        assert!(more > 125.0);
    }

    #[test]
    fn volume_pace_compares_the_same_time_of_day() {
        // Two earlier sessions traded 100 + 100 in their first ten minutes
        // (and much more later); today has traded 400 by then: pace 2.0,
        // not 400 against a full day's average.
        let mut bars = session_bars(26, 50.0, &[100, 100, 5_000, 5_000]);
        bars.extend(session_bars(27, 50.0, &[100, 100, 5_000, 5_000]));
        bars.extend(session_bars(28, 60.0, &[300, 100]));
        let today = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        let s = session_stats(&bars, market::Market::In, today);
        assert!((s.volume_pace.unwrap() - 2.0).abs() < 1e-9, "{s:?}");
        // Today's VWAP only: typical price 60, never the earlier sessions'.
        assert!((s.vwap.unwrap() - 60.0).abs() < 1e-9);
        // One earlier session is not enough to know the pace.
        let s = session_stats(&bars[4..], market::Market::In, today);
        assert_eq!(s.volume_pace, None);
        // Before today's first bar: nothing known.
        let s = session_stats(&bars[..8], market::Market::In, today);
        assert_eq!(s, SessionStats::default());
    }

    #[test]
    fn test_sector_heat_struct() {
        let sh = SectorHeat {
            name: "IT".into(),
            change_pct: 1.5,
            hot: true,
        };
        assert!(sh.hot);
        assert_eq!(sh.name, "IT");
    }

    #[test]
    fn test_clone_signal() {
        let s = make_signal(70.0, Confidence::Medium, 2.0);
        let c = s.clone_signal();
        assert_eq!(c.symbol, s.symbol);
        assert_eq!(c.price, s.price);
        assert_eq!(c.score, s.score);
    }

    #[test]
    fn custom_rule_votes_on_the_latest_bar() {
        use crate::backtest::{Bars, rules::StrategyDef};
        let rule = StrategyDef::parse(
            r#"entry = "close > sma(3)"
               exit = "close < sma(3)""#,
            "up",
            "test",
        )
        .unwrap()
        .compile()
        .unwrap();
        let vote = |closes: &[f64]| rule_vote(&rule, &Bars::from_closes(closes)).unwrap();
        let up = vote(&[10.0, 10.0, 10.0, 12.0]).unwrap();
        assert_eq!(up.direction, Direction::Long);
        assert!(up.reason.starts_with("up: entry"));
        let down = vote(&[10.0, 10.0, 10.0, 8.0]).unwrap();
        assert_eq!(down.direction, Direction::Short);
        assert!(vote(&[10.0; 4]).is_none());
        // Not enough history for the rule's indicators: no vote.
        assert!(vote(&[10.0, 12.0]).is_none());
    }

    #[test]
    fn test_plans_never_exceed_total_capital() {
        // Ten strong signals, each wanting up to 30%: together they would
        // need far more than the capital.
        let signals: Vec<IntradaySignal> = (0..10)
            .map(|i| {
                let mut s = make_signal(90.0, Confidence::High, 3.0);
                s.symbol = format!("S{i}.NS");
                s
            })
            .collect();
        let limits = PlanLimits {
            max_risk_pct: 5.0,
            max_position_pct: 30.0,
            max_positions: 10,
            track_record: None,
            costs: None,
        };
        let plans = generate_trade_plans_with(&signals, 25000.0, limits);
        assert!(!plans.is_empty());
        let total: f64 = plans.iter().map(|p| p.capital_required).sum();
        assert!(total <= 25000.0 + 1e-9, "deployed {total}");
        for p in &plans {
            assert!(p.capital_required <= 25000.0 * 0.30 + 1e-9);
            assert_eq!(p.capital_required, p.qty as f64 * p.entry);
        }
    }

    #[test]
    fn test_stop_is_always_on_the_losing_side_of_the_entry() {
        // Support above the price (an oversold bounce) must not become the stop.
        let mut long = make_signal(80.0, Confidence::High, 2.0);
        long.support1 = Some(101.0);
        let plans = generate_trade_plans(&[long], 25000.0, 1.0);
        assert!(plans[0].stop_loss < plans[0].entry);

        let mut short = make_signal(80.0, Confidence::High, 2.0);
        short.direction = Direction::Short;
        short.resistance1 = Some(99.0);
        let plans = generate_trade_plans(&[short], 25000.0, 1.0);
        assert!(plans[0].stop_loss > plans[0].entry);
    }

    #[test]
    fn test_max_position_pct_is_honoured() {
        let signal = make_signal(90.0, Confidence::High, 3.0);
        let limits = PlanLimits {
            max_risk_pct: 50.0,
            max_position_pct: 10.0,
            max_positions: 5,
            track_record: None,
            costs: None,
        };
        let plans = generate_trade_plans_with(&[signal], 25000.0, limits);
        assert_eq!(plans.len(), 1);
        assert!(plans[0].capital_required <= 2500.0);
    }

    #[test]
    fn test_overflowing_plan_is_shrunk_then_later_ones_dropped() {
        let mk = |sym: &str, qty: u32| TradePlan {
            signal: {
                let mut s = make_signal(80.0, Confidence::High, 2.0);
                s.symbol = sym.into();
                s
            },
            entry: 100.0,
            target1: 101.0,
            target2: 102.0,
            stop_loss: 99.0,
            trailing_stop: 98.0,
            qty,
            capital_required: qty as f64 * 100.0,
            max_risk: qty as f64,
            expected_profit: qty as f64 * 2.0,
            risk_reward: 2.0,
            kelly_fraction: 0.2,
            position_pct: 0.0,
        };
        let plans = vec![mk("A", 6), mk("B", 6), mk("C", 6)];
        let kept = cap_total_allocation(plans, 1000.0, 5, None);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].qty, 6);
        assert_eq!(kept[1].qty, 4); // only ₹400 left
        assert!((kept[1].max_risk - 4.0).abs() < 1e-9);
        assert!((kept[1].expected_profit - 8.0).abs() < 1e-9);
        let kept = cap_total_allocation(vec![mk("A", 1), mk("B", 1), mk("C", 1)], 1000.0, 2, None);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn signal_direction_is_described_not_called() {
        assert_eq!(Direction::Long.lean(), "bullish");
        assert_eq!(Direction::Short.lean(), "bearish");
    }
}
