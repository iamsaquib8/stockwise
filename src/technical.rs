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
    if denom == 0.0 {
        return None;
    }
    Some(cov / denom)
}
