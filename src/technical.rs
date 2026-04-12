/// Calculate Simple Moving Average for the last `period` data points
pub fn sma(data: &[f64], period: usize) -> Option<f64> {
    if data.len() < period {
        return None;
    }
    let slice = &data[data.len() - period..];
    Some(slice.iter().sum::<f64>() / period as f64)
}

/// Calculate Exponential Moving Average
pub fn ema(data: &[f64], period: usize) -> Option<f64> {
    if data.len() < period {
        return None;
    }
    let multiplier = 2.0 / (period as f64 + 1.0);
    // Start with SMA of first `period` values
    let mut ema_val = data[..period].iter().sum::<f64>() / period as f64;
    for &price in &data[period..] {
        ema_val = (price - ema_val) * multiplier + ema_val;
    }
    Some(ema_val)
}

/// Calculate RSI (Relative Strength Index)
pub fn rsi(data: &[f64], period: usize) -> Option<f64> {
    if data.len() < period + 1 {
        return None;
    }

    let mut gains = Vec::new();
    let mut losses = Vec::new();

    for i in 1..data.len() {
        let change = data[i] - data[i - 1];
        if change > 0.0 {
            gains.push(change);
            losses.push(0.0);
        } else {
            gains.push(0.0);
            losses.push(change.abs());
        }
    }

    if gains.len() < period {
        return None;
    }

    let mut avg_gain = gains[..period].iter().sum::<f64>() / period as f64;
    let mut avg_loss = losses[..period].iter().sum::<f64>() / period as f64;

    for i in period..gains.len() {
        avg_gain = (avg_gain * (period as f64 - 1.0) + gains[i]) / period as f64;
        avg_loss = (avg_loss * (period as f64 - 1.0) + losses[i]) / period as f64;
    }

    if avg_loss == 0.0 {
        return Some(100.0);
    }

    let rs = avg_gain / avg_loss;
    Some(100.0 - (100.0 / (1.0 + rs)))
}

/// Calculate MACD (Moving Average Convergence Divergence)
/// Returns (macd_line, signal_line, histogram)
pub fn macd(data: &[f64]) -> Option<(f64, f64, f64)> {
    let ema12 = ema(data, 12)?;
    let ema26 = ema(data, 26)?;
    let macd_line = ema12 - ema26;

    // For signal line, we need MACD history
    if data.len() < 35 {
        return Some((macd_line, 0.0, macd_line));
    }

    let mut macd_history = Vec::new();
    for i in 26..=data.len() {
        let slice = &data[..i];
        if let (Some(e12), Some(e26)) = (ema(slice, 12), ema(slice, 26)) {
            macd_history.push(e12 - e26);
        }
    }

    let signal = ema(&macd_history, 9).unwrap_or(0.0);
    let histogram = macd_line - signal;

    Some((macd_line, signal, histogram))
}

/// Calculate Bollinger Bands
/// Returns (upper, middle, lower)
pub fn bollinger_bands(data: &[f64], period: usize) -> Option<(f64, f64, f64)> {
    if data.len() < period {
        return None;
    }
    let slice = &data[data.len() - period..];
    let middle = slice.iter().sum::<f64>() / period as f64;
    let variance = slice.iter().map(|x| (x - middle).powi(2)).sum::<f64>() / period as f64;
    let std_dev = variance.sqrt();
    Some((middle + 2.0 * std_dev, middle, middle - 2.0 * std_dev))
}

/// Calculate Average True Range (volatility indicator)
pub fn atr(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> Option<f64> {
    if highs.len() < period + 1 || lows.len() < period + 1 || closes.len() < period + 1 {
        return None;
    }

    let mut true_ranges = Vec::new();
    for i in 1..highs.len() {
        let tr = (highs[i] - lows[i])
            .max((highs[i] - closes[i - 1]).abs())
            .max((lows[i] - closes[i - 1]).abs());
        true_ranges.push(tr);
    }

    if true_ranges.len() < period {
        return None;
    }

    let mut atr_val = true_ranges[..period].iter().sum::<f64>() / period as f64;
    for &tr in &true_ranges[period..] {
        atr_val = (atr_val * (period as f64 - 1.0) + tr) / period as f64;
    }

    Some(atr_val)
}

/// Calculate VWAP approximation from typical price * volume
pub fn vwap(highs: &[f64], lows: &[f64], closes: &[f64], volumes: &[u64]) -> Option<f64> {
    if highs.is_empty() {
        return None;
    }
    let mut cum_tp_vol = 0.0;
    let mut cum_vol = 0.0;
    for i in 0..highs.len() {
        let tp = (highs[i] + lows[i] + closes[i]) / 3.0;
        cum_tp_vol += tp * volumes[i] as f64;
        cum_vol += volumes[i] as f64;
    }
    if cum_vol == 0.0 {
        return None;
    }
    Some(cum_tp_vol / cum_vol)
}

/// Generate a signal interpretation
pub fn rsi_signal(rsi_val: f64) -> &'static str {
    match rsi_val {
        x if x >= 80.0 => "Extremely Overbought - Strong sell signal",
        x if x >= 70.0 => "Overbought - Consider selling",
        x if x >= 60.0 => "Bullish momentum",
        x if x >= 40.0 => "Neutral",
        x if x >= 30.0 => "Bearish momentum",
        x if x >= 20.0 => "Oversold - Consider buying",
        _ => "Extremely Oversold - Strong buy signal",
    }
}

pub fn macd_signal(histogram: f64) -> &'static str {
    if histogram > 0.0 {
        "Bullish - MACD above signal line"
    } else {
        "Bearish - MACD below signal line"
    }
}

// ── Risk & Statistical Functions ──

/// Daily returns from price series
pub fn daily_returns(prices: &[f64]) -> Vec<f64> {
    prices
        .windows(2)
        .map(|w| (w[1] - w[0]) / w[0])
        .collect()
}

/// Annualized volatility from daily prices
pub fn annualized_volatility(prices: &[f64]) -> Option<f64> {
    let returns = daily_returns(prices);
    if returns.len() < 2 {
        return None;
    }
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance =
        returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (returns.len() - 1) as f64;
    Some(variance.sqrt() * (252.0_f64).sqrt())
}

/// Sharpe Ratio (annualized, assuming risk-free rate)
pub fn sharpe_ratio(prices: &[f64], risk_free_annual: f64) -> Option<f64> {
    let returns = daily_returns(prices);
    if returns.len() < 2 {
        return None;
    }
    let mean_daily = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance =
        returns.iter().map(|r| (r - mean_daily).powi(2)).sum::<f64>() / (returns.len() - 1) as f64;
    let std_daily = variance.sqrt();
    if std_daily == 0.0 {
        return None;
    }
    let annual_return = mean_daily * 252.0;
    let annual_std = std_daily * (252.0_f64).sqrt();
    Some((annual_return - risk_free_annual) / annual_std)
}

/// Sortino Ratio (only penalizes downside volatility)
pub fn sortino_ratio(prices: &[f64], risk_free_annual: f64) -> Option<f64> {
    let returns = daily_returns(prices);
    if returns.len() < 2 {
        return None;
    }
    let mean_daily = returns.iter().sum::<f64>() / returns.len() as f64;
    let downside: Vec<f64> = returns.iter().filter(|&&r| r < 0.0).copied().collect();
    if downside.is_empty() {
        return None;
    }
    let downside_var = downside.iter().map(|r| r.powi(2)).sum::<f64>() / downside.len() as f64;
    let downside_std = downside_var.sqrt() * (252.0_f64).sqrt();
    if downside_std == 0.0 {
        return None;
    }
    let annual_return = mean_daily * 252.0;
    Some((annual_return - risk_free_annual) / downside_std)
}

/// Maximum drawdown from peak
pub fn max_drawdown(prices: &[f64]) -> Option<(f64, usize, usize)> {
    if prices.len() < 2 {
        return None;
    }
    let mut peak = prices[0];
    let mut max_dd = 0.0_f64;
    let mut peak_idx = 0;
    let mut trough_idx = 0;
    let mut current_peak_idx = 0;

    for (i, &price) in prices.iter().enumerate() {
        if price > peak {
            peak = price;
            current_peak_idx = i;
        }
        let dd = (peak - price) / peak;
        if dd > max_dd {
            max_dd = dd;
            peak_idx = current_peak_idx;
            trough_idx = i;
        }
    }
    Some((max_dd, peak_idx, trough_idx))
}

/// Value at Risk (historical, percentile-based)
pub fn value_at_risk(prices: &[f64], confidence: f64) -> Option<f64> {
    let mut returns = daily_returns(prices);
    if returns.len() < 10 {
        return None;
    }
    returns.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((1.0 - confidence) * returns.len() as f64).floor() as usize;
    Some(returns[idx.min(returns.len() - 1)])
}

/// Calmar Ratio (annualized return / max drawdown)
pub fn calmar_ratio(prices: &[f64]) -> Option<f64> {
    if prices.len() < 2 {
        return None;
    }
    let total_return = (prices.last()? / prices.first()?) - 1.0;
    let days = prices.len() as f64;
    let annual_return = (1.0 + total_return).powf(252.0 / days) - 1.0;
    let (mdd, _, _) = max_drawdown(prices)?;
    if mdd == 0.0 {
        return None;
    }
    Some(annual_return / mdd)
}

/// Pearson correlation between two price series
pub fn correlation(a: &[f64], b: &[f64]) -> Option<f64> {
    let ra = daily_returns(a);
    let rb = daily_returns(b);
    let n = ra.len().min(rb.len());
    if n < 2 {
        return None;
    }
    let ra = &ra[ra.len() - n..];
    let rb = &rb[rb.len() - n..];

    let mean_a = ra.iter().sum::<f64>() / n as f64;
    let mean_b = rb.iter().sum::<f64>() / n as f64;

    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;
    for i in 0..n {
        let da = ra[i] - mean_a;
        let db = rb[i] - mean_b;
        cov += da * db;
        var_a += da * da;
        var_b += db * db;
    }
    let denom = (var_a * var_b).sqrt();
    if denom < f64::EPSILON {
        return None;
    }
    Some(cov / denom)
}

// ── Pivot Points & Fibonacci ──

/// Classic pivot points: (pivot, r1, r2, r3, s1, s2, s3)
pub fn pivot_points(high: f64, low: f64, close: f64) -> (f64, f64, f64, f64, f64, f64, f64) {
    let pivot = (high + low + close) / 3.0;
    let r1 = 2.0 * pivot - low;
    let s1 = 2.0 * pivot - high;
    let r2 = pivot + (high - low);
    let s2 = pivot - (high - low);
    let r3 = high + 2.0 * (pivot - low);
    let s3 = low - 2.0 * (high - pivot);
    (pivot, r1, r2, r3, s1, s2, s3)
}

/// Fibonacci retracement levels from swing high/low
/// Returns: (23.6%, 38.2%, 50%, 61.8%, 78.6%) levels
pub fn fibonacci_levels(swing_high: f64, swing_low: f64) -> [f64; 5] {
    let range = swing_high - swing_low;
    [
        swing_high - range * 0.236,
        swing_high - range * 0.382,
        swing_high - range * 0.500,
        swing_high - range * 0.618,
        swing_high - range * 0.786,
    ]
}

/// Find the most recent swing high and swing low in a price series
pub fn find_swing_points(highs: &[f64], lows: &[f64], lookback: usize) -> Option<(f64, f64)> {
    if highs.len() < lookback || lows.len() < lookback {
        return None;
    }
    let n = highs.len();
    let recent_highs = &highs[n - lookback..];
    let recent_lows = &lows[n - lookback..];
    let swing_high = recent_highs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let swing_low = recent_lows.iter().cloned().fold(f64::INFINITY, f64::min);
    Some((swing_high, swing_low))
}

// ── Volume Analysis ──

/// On-Balance Volume
pub fn obv(closes: &[f64], volumes: &[u64]) -> Vec<f64> {
    let n = closes.len().min(volumes.len());
    let mut result = Vec::with_capacity(n);
    if n == 0 { return result; }
    result.push(volumes[0] as f64);
    for i in 1..n {
        let prev = *result.last().unwrap();
        if closes[i] > closes[i - 1] {
            result.push(prev + volumes[i] as f64);
        } else if closes[i] < closes[i - 1] {
            result.push(prev - volumes[i] as f64);
        } else {
            result.push(prev);
        }
    }
    result
}

/// Accumulation/Distribution Line
pub fn ad_line(highs: &[f64], lows: &[f64], closes: &[f64], volumes: &[u64]) -> Vec<f64> {
    let n = closes.len().min(highs.len()).min(lows.len()).min(volumes.len());
    let mut result = Vec::with_capacity(n);
    let mut ad = 0.0_f64;
    for i in 0..n {
        let hl = highs[i] - lows[i];
        let mfm = if hl > f64::EPSILON {
            ((closes[i] - lows[i]) - (highs[i] - closes[i])) / hl
        } else {
            0.0
        };
        ad += mfm * volumes[i] as f64;
        result.push(ad);
    }
    result
}

// ── Gap Detection ──

#[derive(Debug)]
pub struct Gap {
    pub index: usize,
    pub gap_type: GapType,
    pub gap_low: f64,  // bottom of gap
    pub gap_high: f64, // top of gap
    pub filled: bool,
}

#[derive(Debug)]
pub enum GapType { Up, Down }

/// Detect price gaps between consecutive candles
pub fn detect_gaps(opens: &[f64], highs: &[f64], lows: &[f64], closes: &[f64]) -> Vec<Gap> {
    let n = opens.len().min(highs.len()).min(lows.len()).min(closes.len());
    let mut gaps = Vec::new();
    for i in 1..n {
        // Gap up: today's low > yesterday's high
        if lows[i] > highs[i - 1] {
            let gap_low = highs[i - 1];
            let gap_high = lows[i];
            // Check if gap was filled in subsequent candles
            let filled = (i + 1..n).any(|j| lows[j] <= gap_low);
            gaps.push(Gap { index: i, gap_type: GapType::Up, gap_low, gap_high, filled });
        }
        // Gap down: today's high < yesterday's low
        if highs[i] < lows[i - 1] {
            let gap_low = highs[i];
            let gap_high = lows[i - 1];
            let filled = (i + 1..n).any(|j| highs[j] >= gap_high);
            gaps.push(Gap { index: i, gap_type: GapType::Down, gap_low, gap_high, filled });
        }
    }
    gaps
}

/// ATR-based stop loss levels
/// Returns (conservative, moderate, aggressive) stop prices for a long position
pub fn atr_stop_loss(entry: f64, atr: f64) -> (f64, f64, f64) {
    (
        entry - 3.0 * atr, // conservative: 3x ATR
        entry - 2.0 * atr, // moderate: 2x ATR
        entry - 1.5 * atr, // aggressive: 1.5x ATR
    )
}

/// Chandelier exit (for trailing stop)
pub fn chandelier_exit(highs: &[f64], atr: f64, multiplier: f64) -> Option<f64> {
    let max_high = highs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    Some(max_high - multiplier * atr)
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sma() {
        assert_eq!(sma(&[1.0, 2.0, 3.0, 4.0, 5.0], 3), Some(4.0));
        assert_eq!(sma(&[1.0, 2.0], 3), None);
        assert_eq!(sma(&[10.0, 10.0, 10.0], 3), Some(10.0));
        assert_eq!(sma(&[], 3), None);
    }

    #[test]
    fn test_ema() {
        let data = vec![10.0, 11.0, 12.0, 13.0, 14.0];
        let result = ema(&data, 3);
        assert!(result.is_some());
        // EMA(3) on [10,11,12,13,14]: SMA start=11, then apply EMA formula
        let r = result.unwrap();
        assert!(r > 12.0 && r < 15.0, "EMA should be between 12 and 15, got {}", r);
        assert_eq!(ema(&[1.0], 3), None);
    }

    #[test]
    fn test_rsi() {
        // Monotonically increasing = RSI 100
        let up = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0];
        assert_eq!(rsi(&up, 14), Some(100.0));
        // Too short
        assert_eq!(rsi(&[1.0, 2.0], 14), None);
    }

    #[test]
    fn test_rsi_range() {
        let data: Vec<f64> = (0..100).map(|i| 100.0 + (i as f64 * 0.1).sin() * 10.0).collect();
        let r = rsi(&data, 14).unwrap();
        assert!(r >= 0.0 && r <= 100.0);
    }

    #[test]
    fn test_bollinger_bands() {
        let data = vec![10.0; 20];
        let (upper, middle, lower) = bollinger_bands(&data, 20).unwrap();
        assert!((middle - 10.0).abs() < f64::EPSILON);
        assert!((upper - 10.0).abs() < f64::EPSILON); // zero std dev
        assert!((lower - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_macd() {
        let data: Vec<f64> = (0..50).map(|i| 100.0 + i as f64).collect();
        let result = macd(&data);
        assert!(result.is_some());
        let (line, _signal, _hist) = result.unwrap();
        assert!(line > 0.0); // uptrend = positive MACD
    }

    #[test]
    fn test_max_drawdown() {
        let prices = vec![100.0, 110.0, 90.0, 95.0, 80.0, 100.0];
        let (dd, peak, trough) = max_drawdown(&prices).unwrap();
        assert!((dd - (110.0 - 80.0) / 110.0).abs() < 0.001);
        assert_eq!(peak, 1); // 110.0
        assert_eq!(trough, 4); // 80.0
    }

    #[test]
    fn test_sharpe_ratio() {
        let prices: Vec<f64> = (0..252).map(|i| 100.0 + i as f64 * 0.1).collect();
        let s = sharpe_ratio(&prices, 0.0);
        assert!(s.is_some());
        assert!(s.unwrap() > 0.0);
    }

    #[test]
    fn test_value_at_risk() {
        let prices: Vec<f64> = (0..100).map(|i| 100.0 + (i as f64 * 0.3).sin() * 5.0).collect();
        let var95 = value_at_risk(&prices, 0.95);
        assert!(var95.is_some());
        assert!(var95.unwrap() < 0.0); // VaR should be negative
    }

    #[test]
    fn test_correlation_perfect() {
        let a = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let b = vec![2.0, 4.0, 6.0, 8.0, 10.0];
        let corr = correlation(&a, &b).unwrap();
        assert!((corr - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_correlation_negative() {
        // a zigzags up-down, b zigzags down-up (opposite returns)
        let a = vec![100.0, 110.0, 100.0, 110.0, 100.0, 110.0];
        let b = vec![100.0, 90.0, 100.0, 90.0, 100.0, 90.0];
        let corr = correlation(&a, &b).unwrap();
        assert!((corr - (-1.0)).abs() < 0.01, "Expected -1.0 correlation, got {}", corr);
    }

    #[test]
    fn test_pivot_points() {
        let (p, r1, r2, _r3, s1, s2, _s3) = pivot_points(110.0, 90.0, 100.0);
        assert!((p - 100.0).abs() < f64::EPSILON);
        assert!(r1 > p);
        assert!(r2 > r1);
        assert!(s1 < p);
        assert!(s2 < s1);
    }

    #[test]
    fn test_fibonacci_levels() {
        let levels = fibonacci_levels(100.0, 50.0);
        assert!((levels[0] - 88.2).abs() < 0.1); // 23.6%
        assert!((levels[1] - 80.9).abs() < 0.1); // 38.2%
        assert!((levels[2] - 75.0).abs() < 0.1); // 50%
        assert!((levels[3] - 69.1).abs() < 0.1); // 61.8%
        assert!((levels[4] - 60.7).abs() < 0.1); // 78.6%
    }

    #[test]
    fn test_obv() {
        let closes = vec![10.0, 11.0, 10.5, 11.5, 11.0];
        let volumes = vec![100, 200, 150, 300, 100];
        let result = obv(&closes, &volumes);
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], 100.0);
        assert_eq!(result[1], 300.0); // up
        assert_eq!(result[2], 150.0); // down
        assert_eq!(result[3], 450.0); // up
        assert_eq!(result[4], 350.0); // down
    }

    #[test]
    fn test_gap_detection() {
        let opens = vec![100.0, 110.0, 105.0];
        let highs = vec![105.0, 115.0, 110.0];
        let lows = vec![95.0, 108.0, 100.0];
        let closes = vec![102.0, 112.0, 108.0];
        let gaps = detect_gaps(&opens, &highs, &lows, &closes);
        assert_eq!(gaps.len(), 1); // gap up: low[1]=108 > high[0]=105
    }

    #[test]
    fn test_atr_stop_loss() {
        let (cons, mod_, agg) = atr_stop_loss(100.0, 5.0);
        assert!((cons - 85.0).abs() < f64::EPSILON);
        assert!((mod_ - 90.0).abs() < f64::EPSILON);
        assert!((agg - 92.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_daily_returns() {
        let prices = vec![100.0, 110.0, 99.0];
        let returns = daily_returns(&prices);
        assert_eq!(returns.len(), 2);
        assert!((returns[0] - 0.1).abs() < f64::EPSILON);
        assert!((returns[1] - (-0.1)).abs() < 0.001);
    }

    #[test]
    fn test_annualized_volatility() {
        // Constant price = zero volatility
        let flat = vec![100.0; 50];
        assert_eq!(annualized_volatility(&flat), Some(0.0));
    }
}
