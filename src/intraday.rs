use crate::api::{Quote, YahooClient};
use crate::market;
use crate::technical;
use anyhow::Result;

/// Signals derived from intraday + short-term data for a stock
#[derive(Debug)]
pub struct IntradaySignal {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub currency: Option<String>,
    pub change_pct: f64,
    pub volume_ratio: f64,     // today's volume vs 3-month avg
    pub rsi: Option<f64>,
    pub vwap: Option<f64>,
    pub above_vwap: bool,
    pub sma_20_trend: Option<f64>, // price distance from SMA20 in %
    pub macd_histogram: Option<f64>,
    pub bb_position: Option<f64>,  // 0.0 = at lower band, 1.0 = at upper band
    pub atr_pct: Option<f64>,      // ATR as % of price (daily range expectation)
    pub score: f64,                // composite score 0-100
    pub direction: Direction,
}

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

/// A concrete trade suggestion
#[derive(Debug)]
pub struct TradePlan {
    pub signal: IntradaySignal,
    pub entry: f64,
    pub target: f64,
    pub stop_loss: f64,
    pub qty: u32,
    pub capital_required: f64,
    pub expected_profit: f64,
    pub risk_reward: f64,
}

/// Scan stocks and generate intraday signals
pub async fn scan_intraday(client: &YahooClient) -> Result<Vec<IntradaySignal>> {
    let symbols = market::INDIA_POPULAR;
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

        // Fetch 1mo daily data for technicals
        let chart = match client.get_chart(&symbol, "1mo", "1d").await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let closes: Vec<f64> = chart
            .indicators
            .quote
            .first()
            .and_then(|q| q.close.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();

        let highs: Vec<f64> = chart
            .indicators
            .quote
            .first()
            .and_then(|q| q.high.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();

        let lows: Vec<f64> = chart
            .indicators
            .quote
            .first()
            .and_then(|q| q.low.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();

        let volumes: Vec<u64> = chart
            .indicators
            .quote
            .first()
            .and_then(|q| q.volume.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();

        if closes.len() < 15 {
            continue;
        }

        let signal = compute_signal(
            &symbol,
            quote,
            price,
            &closes,
            &highs,
            &lows,
            &volumes,
        );
        signals.push(signal);
    }

    // Sort by score descending
    signals.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    Ok(signals)
}

fn compute_signal(
    symbol: &str,
    quote: &Quote,
    price: f64,
    closes: &[f64],
    highs: &[f64],
    lows: &[f64],
    volumes: &[u64],
) -> IntradaySignal {
    let mut score = 50.0_f64; // start neutral

    // Volume ratio
    let avg_vol = quote.average_daily_volume_3_month.unwrap_or(1) as f64;
    let today_vol = quote.regular_market_volume.unwrap_or(0) as f64;
    let volume_ratio = if avg_vol > 0.0 {
        today_vol / avg_vol
    } else {
        1.0
    };
    // High volume = more conviction
    if volume_ratio > 1.5 {
        score += 10.0;
    } else if volume_ratio > 1.2 {
        score += 5.0;
    } else if volume_ratio < 0.5 {
        score -= 10.0; // low volume = avoid
    }

    // RSI
    let rsi = technical::rsi(closes, 14);
    if let Some(r) = rsi {
        if r < 35.0 {
            score += 15.0; // oversold bounce
        } else if r < 45.0 {
            score += 8.0;
        } else if r > 75.0 {
            score -= 10.0; // overbought, risky long
        }
    }

    // VWAP
    let vwap = technical::vwap(highs, lows, closes, volumes);
    let above_vwap = vwap.is_some_and(|v| price > v);
    if above_vwap {
        score += 5.0; // above VWAP = bullish intraday
    }

    // SMA 20 trend
    let sma20 = technical::sma(closes, 20);
    let sma_20_trend = sma20.map(|ma| ((price - ma) / ma) * 100.0);
    if let Some(dist) = sma_20_trend {
        if dist > 0.0 && dist < 3.0 {
            score += 8.0; // just above support
        } else if dist < -5.0 {
            score += 5.0; // mean reversion opportunity
        } else if dist > 8.0 {
            score -= 5.0; // too extended
        }
    }

    // MACD
    let macd = technical::macd(closes);
    let macd_histogram = macd.map(|(_, _, h)| h);
    if let Some(h) = macd_histogram {
        if h > 0.0 {
            score += 5.0; // bullish momentum
        }
        // Histogram just turned positive = fresh crossover
        // We'd need history for this, approximate with small positive
        if h > 0.0 && h < 1.0 {
            score += 5.0;
        }
    }

    // Bollinger Band position
    let bb = technical::bollinger_bands(closes, 20);
    let bb_position = bb.map(|(upper, _, lower)| {
        if upper == lower {
            0.5
        } else {
            (price - lower) / (upper - lower)
        }
    });
    if let Some(pos) = bb_position {
        if pos < 0.2 {
            score += 10.0; // near lower band = bounce candidate
        } else if pos > 0.9 {
            score -= 5.0; // near upper band
        }
    }

    // ATR (expected daily range)
    let atr = technical::atr(highs, lows, closes, 14);
    let atr_pct = atr.map(|a| (a / price) * 100.0);

    // Today's change — mild pullback is good entry for long
    let change_pct = quote.regular_market_change_percent.unwrap_or(0.0);
    if change_pct > -2.0 && change_pct < -0.2 {
        score += 5.0; // mild dip = entry
    } else if change_pct < -4.0 {
        score -= 5.0; // falling knife
    }

    // Direction: mostly long for intraday (simpler and more common)
    let direction = Direction::Long;

    // Clamp score
    score = score.clamp(0.0, 100.0);

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
        rsi,
        vwap,
        above_vwap,
        sma_20_trend,
        macd_histogram,
        bb_position,
        atr_pct,
        score,
        direction,
    }
}

/// Generate trade plans from signals to achieve a target profit on a given capital
pub fn generate_trade_plans(
    signals: &[IntradaySignal],
    capital: f64,
    target_pct: f64,
) -> Vec<TradePlan> {
    let target_profit = capital * (target_pct / 100.0);
    let mut plans = Vec::new();

    for signal in signals.iter().filter(|s| s.score >= 55.0) {
        let price = signal.price;

        // Use ATR to set realistic target and stop-loss
        let atr_pct = signal.atr_pct.unwrap_or(2.0);
        // Target: aim for 40-60% of daily ATR as intraday move
        let move_pct = atr_pct * 0.5;

        if move_pct < 0.3 {
            continue; // stock doesn't move enough
        }

        let entry = price;
        let target = match signal.direction {
            Direction::Long => price * (1.0 + move_pct / 100.0),
            Direction::Short => price * (1.0 - move_pct / 100.0),
        };
        // Stop-loss: risk half of what we aim to gain (2:1 reward/risk)
        let stop_loss = match signal.direction {
            Direction::Long => price * (1.0 - (move_pct / 200.0)),
            Direction::Short => price * (1.0 + (move_pct / 200.0)),
        };

        let profit_per_share = (target - entry).abs();
        let loss_per_share = (entry - stop_loss).abs();
        let risk_reward = if loss_per_share > 0.0 {
            profit_per_share / loss_per_share
        } else {
            0.0
        };

        // How many shares to buy to hit target profit?
        // But also constrained by capital
        let qty_for_profit = if profit_per_share > 0.0 {
            (target_profit / profit_per_share).ceil() as u32
        } else {
            continue;
        };

        let qty_for_capital = (capital / price).floor() as u32;
        let qty = qty_for_profit.min(qty_for_capital);
        if qty == 0 {
            continue;
        }

        let capital_required = qty as f64 * price;
        let expected_profit = qty as f64 * profit_per_share;

        plans.push(TradePlan {
            signal: IntradaySignal {
                symbol: signal.symbol.clone(),
                name: signal.name.clone(),
                price: signal.price,
                currency: signal.currency.clone(),
                change_pct: signal.change_pct,
                volume_ratio: signal.volume_ratio,
                rsi: signal.rsi,
                vwap: signal.vwap,
                above_vwap: signal.above_vwap,
                sma_20_trend: signal.sma_20_trend,
                macd_histogram: signal.macd_histogram,
                bb_position: signal.bb_position,
                atr_pct: signal.atr_pct,
                score: signal.score,
                direction: signal.direction,
            },
            entry,
            target,
            stop_loss,
            qty,
            capital_required,
            expected_profit,
            risk_reward,
        });
    }

    // Sort by how close expected_profit is to target (prefer plans that hit it exactly)
    plans.sort_by(|a, b| {
        let a_diff = (a.expected_profit - target_profit).abs();
        let b_diff = (b.expected_profit - target_profit).abs();
        a_diff.partial_cmp(&b_diff).unwrap()
    });

    plans
}
