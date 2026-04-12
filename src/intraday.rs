use crate::api::{Quote, YahooClient};
use crate::market;
use crate::technical;
use anyhow::Result;

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
            Direction::Long => write!(f, "BUY"),
            Direction::Short => write!(f, "SELL"),
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
    pub volume_ratio: f64,
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

/// Trade plan with Kelly-criterion position sizing
#[derive(Debug)]
pub struct TradePlan {
    pub signal: IntradaySignal,
    pub entry: f64,
    pub target1: f64,      // first target (book 50%)
    pub target2: f64,      // second target (let it ride)
    pub stop_loss: f64,
    pub trailing_stop: f64,
    pub qty: u32,
    pub capital_required: f64,
    pub max_risk: f64,      // max loss in currency
    pub expected_profit: f64,
    pub risk_reward: f64,
    pub kelly_fraction: f64, // optimal bet fraction
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
    let recent = if returns.len() > 10 { &returns[returns.len() - 10..] } else { &returns };
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

/// Run all strategies and compute a composite signal
fn compute_signal(
    symbol: &str,
    quote: &Quote,
    price: f64,
    closes: &[f64],
    highs: &[f64],
    lows: &[f64],
    volumes: &[u64],
    opens: &[f64],
) -> IntradaySignal {
    let mut strategies: Vec<StrategySignal> = Vec::new();
    let regime = detect_regime(closes);

    // ── Strategy 1: RSI Reversal ──
    let rsi = technical::rsi(closes, 14);
    if let Some(r) = rsi {
        if r < 30.0 {
            strategies.push(StrategySignal {
                name: "RSI Reversal",
                direction: Direction::Long,
                strength: 85.0 + (30.0 - r), // stronger as RSI drops
                reason: format!("RSI deeply oversold at {:.0}", r),
            });
        } else if r < 40.0 {
            strategies.push(StrategySignal {
                name: "RSI Reversal",
                direction: Direction::Long,
                strength: 60.0,
                reason: format!("RSI approaching oversold at {:.0}", r),
            });
        } else if r > 70.0 {
            strategies.push(StrategySignal {
                name: "RSI Reversal",
                direction: Direction::Short,
                strength: 60.0 + (r - 70.0),
                reason: format!("RSI overbought at {:.0}", r),
            });
        }
    }

    // ── Strategy 2: VWAP Reclaim ──
    let vwap = technical::vwap(highs, lows, closes, volumes);
    let above_vwap = vwap.is_some_and(|v| price > v);
    if let Some(v) = vwap {
        let dist_pct = ((price - v) / v) * 100.0;
        if dist_pct > 0.0 && dist_pct < 0.5 {
            // Just reclaimed VWAP — strong long signal
            strategies.push(StrategySignal {
                name: "VWAP Reclaim",
                direction: Direction::Long,
                strength: 75.0,
                reason: "Price just reclaimed VWAP from below".into(),
            });
        } else if dist_pct < 0.0 && dist_pct > -0.5 {
            strategies.push(StrategySignal {
                name: "VWAP Rejection",
                direction: Direction::Short,
                strength: 65.0,
                reason: "Price just lost VWAP".into(),
            });
        }
    }

    // ── Strategy 3: Bollinger Band Squeeze & Breakout ──
    let bb = technical::bollinger_bands(closes, 20);
    let bb_position = bb.map(|(upper, _, lower)| {
        if (upper - lower).abs() < f64::EPSILON { 0.5 } else { (price - lower) / (upper - lower) }
    });
    if let Some((upper, middle, lower)) = bb {
        let bandwidth = (upper - lower) / middle * 100.0;
        if bandwidth < 3.0 && price > middle {
            // Squeeze about to break out long
            strategies.push(StrategySignal {
                name: "BB Squeeze",
                direction: Direction::Long,
                strength: 70.0,
                reason: format!("Bollinger squeeze ({:.1}% width), price above middle", bandwidth),
            });
        } else if bb_position.unwrap_or(0.5) < 0.1 {
            strategies.push(StrategySignal {
                name: "BB Bounce",
                direction: Direction::Long,
                strength: 65.0,
                reason: "Price touching lower Bollinger Band".into(),
            });
        } else if bb_position.unwrap_or(0.5) > 0.95 {
            strategies.push(StrategySignal {
                name: "BB Rejection",
                direction: Direction::Short,
                strength: 60.0,
                reason: "Price at upper Bollinger Band".into(),
            });
        }
    }

    // ── Strategy 4: MACD Crossover ──
    let macd = technical::macd(closes);
    let macd_histogram = macd.map(|(_, _, h)| h);
    if closes.len() > 36 {
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

    // ── Strategy 5: Volume Breakout ──
    let avg_vol = quote.average_daily_volume_3_month.unwrap_or(1) as f64;
    let today_vol = quote.regular_market_volume.unwrap_or(0) as f64;
    let volume_ratio = if avg_vol > 0.0 { today_vol / avg_vol } else { 1.0 };
    if volume_ratio > 2.0 && quote.regular_market_change_percent.unwrap_or(0.0) > 0.5 {
        strategies.push(StrategySignal {
            name: "Volume Breakout",
            direction: Direction::Long,
            strength: 80.0,
            reason: format!("Volume {:.1}x average with positive price action", volume_ratio),
        });
    }

    // ── Strategy 6: Support/Resistance Bounce ──
    let n = highs.len().min(lows.len()).min(closes.len());
    let (mut near_support, mut near_resistance) = (false, false);
    let (mut pivot_val, mut s1_val, mut r1_val) = (None, None, None);
    if n > 1 {
        let (pivot, r1, _r2, _r3, s1, _s2, _s3) =
            technical::pivot_points(highs[n - 1], lows[n - 1], closes[n - 1]);
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

    // ── Strategy 7: Gap Play ──
    let gap_detected = if opens.len() >= 2 && highs.len() >= 2 && lows.len() >= 2 {
        let gaps = technical::detect_gaps(opens, highs, lows, closes);
        let recent_unfilled: Vec<_> = gaps.iter().filter(|g| !g.filled && g.index >= opens.len().saturating_sub(5)).collect();
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

    // ── Strategy 8: OBV Divergence ──
    let obv_data = technical::obv(closes, volumes);
    let obv_trend = if obv_data.len() > 10 {
        let obv_sma = technical::sma(&obv_data, 10);
        let last_obv = *obv_data.last().unwrap();
        match obv_sma {
            Some(avg) if last_obv > avg * 1.05 => {
                // OBV rising while price might be flat = accumulation
                if regime == MarketRegime::Ranging || regime == MarketRegime::Downtrend {
                    strategies.push(StrategySignal {
                        name: "OBV Divergence",
                        direction: Direction::Long,
                        strength: 68.0,
                        reason: "OBV rising (accumulation) despite weak price".into(),
                    });
                }
                OBVTrend::Rising
            }
            Some(avg) if last_obv < avg * 0.95 => OBVTrend::Falling,
            _ => OBVTrend::Flat,
        }
    } else {
        OBVTrend::Flat
    };

    // ── Strategy 9: Mean Reversion ──
    if let Some(sma20) = technical::sma(closes, 20) {
        let deviation = ((price - sma20) / sma20) * 100.0;
        if deviation < -3.0 && regime != MarketRegime::StrongDowntrend {
            strategies.push(StrategySignal {
                name: "Mean Reversion",
                direction: Direction::Long,
                strength: 62.0 + deviation.abs().min(5.0) * 2.0,
                reason: format!("Price {:.1}% below 20-SMA, mean reversion expected", deviation),
            });
        }
    }

    // ── Compute Composite Score ──
    let long_signals: Vec<&StrategySignal> = strategies.iter().filter(|s| s.direction == Direction::Long).collect();
    let short_signals: Vec<&StrategySignal> = strategies.iter().filter(|s| s.direction == Direction::Short).collect();

    let long_strength: f64 = long_signals.iter().map(|s| s.strength).sum();
    let short_strength: f64 = short_signals.iter().map(|s| s.strength).sum();

    let (direction, raw_score) = if long_strength > short_strength {
        (Direction::Long, long_strength)
    } else if short_strength > long_strength {
        (Direction::Short, short_strength)
    } else {
        (Direction::Long, 0.0)
    };

    // Normalize: more strategies agreeing = higher score
    let agreeing_count = if direction == Direction::Long { long_signals.len() } else { short_signals.len() };
    let consensus_bonus = (agreeing_count as f64 - 1.0).max(0.0) * 8.0; // bonus for multi-strategy agreement

    // Volume confirmation multiplier
    let vol_mult = if volume_ratio > 1.5 { 1.15 } else if volume_ratio > 1.0 { 1.0 } else { 0.85 };

    // Regime alignment bonus
    let regime_mult = match (direction, &regime) {
        (Direction::Long, MarketRegime::StrongUptrend) => 1.2,
        (Direction::Long, MarketRegime::Uptrend) => 1.1,
        (Direction::Short, MarketRegime::StrongDowntrend) => 1.2,
        (Direction::Short, MarketRegime::Downtrend) => 1.1,
        (Direction::Long, MarketRegime::StrongDowntrend) => 0.6, // counter-trend = penalize
        (Direction::Short, MarketRegime::StrongUptrend) => 0.6,
        _ => 1.0,
    };

    let avg_strength = if agreeing_count > 0 { raw_score / agreeing_count as f64 } else { 0.0 };
    let score = ((avg_strength + consensus_bonus) * vol_mult * regime_mult).clamp(0.0, 100.0);

    let confidence = match (score, agreeing_count) {
        (s, c) if s >= 75.0 && c >= 3 => Confidence::High,
        (s, c) if s >= 60.0 && c >= 2 => Confidence::Medium,
        _ => Confidence::Low,
    };

    let atr = technical::atr(highs, lows, closes, 14);
    let atr_pct = atr.map(|a| (a / price) * 100.0);
    let change_pct = quote.regular_market_change_percent.unwrap_or(0.0);

    IntradaySignal {
        symbol: symbol.to_string(),
        name: quote.short_name.as_deref().or(quote.long_name.as_deref()).unwrap_or("Unknown").to_string(),
        price,
        currency: quote.currency.clone(),
        change_pct,
        volume_ratio,
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

// ══════════════════════════════════════════════════════════
// POSITION SIZING
// ══════════════════════════════════════════════════════════

/// Kelly Criterion for optimal position sizing
/// win_rate: 0-1, avg_win/avg_loss: ratio
fn kelly_fraction(win_rate: f64, win_loss_ratio: f64) -> f64 {
    let kelly = win_rate - (1.0 - win_rate) / win_loss_ratio;
    // Half-Kelly for safety
    (kelly * 0.5).clamp(0.0, 0.25) // never bet more than 25% of capital
}

/// Generate trade plans with intelligent position sizing
pub fn generate_trade_plans(
    signals: &[IntradaySignal],
    capital: f64,
    target_pct: f64,
    max_risk_pct: f64, // max % of capital to risk per trade (e.g., 1.0 = 1%)
) -> Vec<TradePlan> {
    let target_profit = capital * (target_pct / 100.0);
    let max_risk_per_trade = capital * (max_risk_pct / 100.0);
    let mut plans = Vec::new();

    for signal in signals.iter().filter(|s| s.score >= 50.0 && s.confidence != Confidence::Low) {
        let price = signal.price;
        let atr_pct = signal.atr_pct.unwrap_or(2.0);

        if atr_pct < 0.3 {
            continue; // not enough daily range
        }

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

        // Smart stop-loss: use support level if available, else ATR-based
        let atr_stop = match signal.direction {
            Direction::Long => price * (1.0 - atr_pct * 0.4 / 100.0),
            Direction::Short => price * (1.0 + atr_pct * 0.4 / 100.0),
        };
        let stop_loss = if signal.direction == Direction::Long {
            signal.support1.map(|s| s.max(atr_stop)).unwrap_or(atr_stop)
        } else {
            signal.resistance1.map(|r| r.min(atr_stop)).unwrap_or(atr_stop)
        };

        // Trailing stop: 1.5x ATR from current price
        let trailing_stop = match signal.direction {
            Direction::Long => price - (atr_pct * 1.5 / 100.0) * price,
            Direction::Short => price + (atr_pct * 1.5 / 100.0) * price,
        };

        let risk_per_share = (price - stop_loss).abs();
        let profit_per_share = (target2 - price).abs();
        if risk_per_share < f64::EPSILON {
            continue;
        }
        let risk_reward = profit_per_share / risk_per_share;

        // Kelly-based position sizing
        // Estimate win rate from confidence: High=65%, Medium=55%, Low=45%
        let est_win_rate = match signal.confidence {
            Confidence::High => 0.65,
            Confidence::Medium => 0.55,
            Confidence::Low => 0.45,
        };
        let kf = kelly_fraction(est_win_rate, risk_reward);

        // Position size: min of Kelly allocation, max risk constraint, and total capital
        let kelly_capital = capital * kf;
        let risk_capital = max_risk_per_trade / risk_per_share * price;
        let max_capital = capital * 0.3; // never more than 30% in one trade
        let position_capital = kelly_capital.min(risk_capital).min(max_capital);

        let qty = (position_capital / price).floor() as u32;
        if qty == 0 {
            continue;
        }

        let capital_required = qty as f64 * price;
        let max_risk = qty as f64 * risk_per_share;
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

    // Sort by score * risk_reward (best risk-adjusted opportunities first)
    plans.sort_by(|a, b| {
        let a_val = a.signal.score * a.risk_reward;
        let b_val = b.signal.score * b.risk_reward;
        b_val.partial_cmp(&a_val).unwrap()
    });

    plans
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

    heats.sort_by(|a, b| b.change_pct.partial_cmp(&a.change_pct).unwrap());
    Ok(heats)
}

// ══════════════════════════════════════════════════════════
// SCANNER
// ══════════════════════════════════════════════════════════

pub async fn scan_intraday(client: &YahooClient, market: market::Market) -> Result<Vec<IntradaySignal>> {
    let symbols = match market {
        market::Market::In => market::INDIA_POPULAR,
        market::Market::Us => market::US_POPULAR,
    };
    let sym_refs: Vec<&str> = symbols.to_vec();
    let quotes = client.get_quote(&sym_refs).await?;
    let mut signals = Vec::new();

    for quote in &quotes {
        let symbol = match &quote.symbol {
            Some(s) => s.clone(),
            None => continue,
        };
        let price = match quote.regular_market_price {
            Some(p) if p > 0.0 => p,
            _ => continue,
        };

        let chart = match client.get_chart(&symbol, "1mo", "1d").await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let extract = |f: &dyn Fn(&crate::api::QuoteIndicator) -> &Option<Vec<Option<f64>>>| -> Vec<f64> {
            chart.indicators.quote.first().and_then(|q| f(q).as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default()
        };

        let closes: Vec<f64> = extract(&|q| &q.close);
        let highs: Vec<f64> = extract(&|q| &q.high);
        let lows: Vec<f64> = extract(&|q| &q.low);
        let opens: Vec<f64> = extract(&|q| &q.open);
        let volumes: Vec<u64> = chart.indicators.quote.first()
            .and_then(|q| q.volume.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();

        if closes.len() < 15 {
            continue;
        }

        let signal = compute_signal(&symbol, quote, price, &closes, &highs, &lows, &volumes, &opens);
        signals.push(signal);
    }

    signals.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
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
