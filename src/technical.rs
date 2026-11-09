use crate::market::AssetClass;
use chrono::NaiveDate;

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
    ema_iter(data, period).and_then(Iterator::last)
}

/// Returns iterator over exponential moving average values for `period-1..data.len()` range.
pub fn ema_iter(data: &[f64], period: usize) -> Option<impl Iterator<Item = f64>> {
    if data.len() < period || period == 0 {
        return None;
    }

    let multiplier = 2.0 / (period as f64 + 1.0);
    // Start with SMA of first `period` values
    let mut ema_val = data[..period].iter().sum::<f64>() / period as f64;

    Some(
        std::iter::once(ema_val).chain(data[period..].iter().map(move |&price| {
            ema_val = (price - ema_val) * multiplier + ema_val;
            ema_val
        })),
    )
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
    // For signal line, we need MACD history
    if data.len() < 35 {
        let ema26 = ema(data, 26)?;
        let ema12 = ema(data, 12)?;
        let macd_line = ema12 - ema26;
        return Some((macd_line, 0.0, macd_line));
    }

    let ema26_iter = ema_iter(data, 26)?;
    let ema12_iter = ema_iter(data, 12)?.skip(26 - 12);

    let macd_history: Vec<f64> = ema12_iter
        .zip(ema26_iter)
        .map(|(ema12, ema26)| ema12 - ema26)
        .collect();

    let signal = ema(&macd_history, 9).unwrap_or(0.0);
    let macd_line = macd_history.last().copied().expect("shouldn't be empty");
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
    let n = highs.len().min(lows.len()).min(closes.len());
    if n < period + 1 {
        return None;
    }

    let mut true_ranges = Vec::new();
    for i in 1..n {
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
    let n = highs
        .len()
        .min(lows.len())
        .min(closes.len())
        .min(volumes.len());
    if n == 0 {
        return None;
    }
    let mut cum_tp_vol = 0.0;
    let mut cum_vol = 0.0;
    for i in 0..n {
        let tp = (highs[i] + lows[i] + closes[i]) / 3.0;
        cum_tp_vol += tp * volumes[i] as f64;
        cum_vol += volumes[i] as f64;
    }
    if cum_vol == 0.0 {
        return None;
    }
    Some(cum_tp_vol / cum_vol)
}

/// Describe where an RSI reading sits. Words only describe the reading; they
/// are not a trading call.
pub fn rsi_signal(rsi_val: f64) -> &'static str {
    match rsi_val {
        x if x >= 80.0 => "RSI above 80 (extremely overbought zone)",
        x if x >= 70.0 => "RSI above 70 (overbought zone)",
        x if x >= 60.0 => "Bullish momentum",
        x if x >= 40.0 => "Neutral",
        x if x >= 30.0 => "Bearish momentum",
        x if x >= 20.0 => "RSI below 30 (oversold zone)",
        _ => "RSI below 20 (extremely oversold zone)",
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

/// Percent change from `start` to `end`.
///
/// Returns `0.0` when `start` is not positive so callers never print `+inf%`
/// or NaN from a zero/negative initial price (#59).
pub fn percent_change(start: f64, end: f64) -> f64 {
    if start > 0.0 {
        (end / start - 1.0) * 100.0
    } else {
        0.0
    }
}

/// Daily returns from price series
pub fn daily_returns(prices: &[f64]) -> Vec<f64> {
    prices
        .windows(2)
        .filter(|w| w[0] != 0.0)
        .map(|w| (w[1] - w[0]) / w[0])
        .collect()
}

/// Annualized volatility from daily prices
pub fn annualized_volatility(prices: &[f64]) -> Option<f64> {
    annualized_volatility_with(prices, 252.0)
}

/// Annualized volatility from prices sampled `periods_per_year` times a year
/// (252 for daily bars, 52 weekly, 12 monthly).
pub fn annualized_volatility_with(prices: &[f64], periods_per_year: f64) -> Option<f64> {
    let returns = daily_returns(prices);
    if returns.len() < 2 {
        return None;
    }
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance =
        returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (returns.len() - 1) as f64;
    Some(variance.sqrt() * periods_per_year.sqrt())
}

/// Annualized internal rate of return (XIRR) of `cashflows`, given as
/// `(time in years, amount)` with investments negative and the final value
/// positive. Solved by bisection; `None` if there is no sign change.
pub fn xirr(cashflows: &[(f64, f64)]) -> Option<f64> {
    let npv = |r: f64| -> f64 {
        cashflows
            .iter()
            .map(|&(t, cf)| cf / (1.0 + r).powf(t))
            .sum()
    };
    let (mut lo, mut hi) = (-0.9999, 100.0);
    let (f_lo, f_hi) = (npv(lo), npv(hi));
    if !f_lo.is_finite() || !f_hi.is_finite() || f_lo.signum() == f_hi.signum() {
        return None;
    }
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        let f_mid = npv(mid);
        if f_mid.abs() < 1e-9 {
            return Some(mid);
        }
        if f_mid.signum() == f_lo.signum() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / 2.0)
}

/// Bars per year for a Yahoo chart interval of a stock on a 6.25-hour (NSE)
/// session. Use [`periods_per_year_for`] for other asset classes.
pub fn periods_per_year(interval: &str) -> f64 {
    periods_per_year_for(AssetClass::Stock, interval, 6.25)
}

/// Bars per year for a Yahoo chart interval, by asset class. Daily bars: 252
/// trading days for stocks and indices, 365 for crypto, which trades every
/// day, 260 for FX (weekdays). Intraday bars fill `session_hours` of each
/// trading day (the exchange's session), or 24 hours for crypto and FX.
/// Weekly and monthly bars are 52 and 12 a year.
pub fn periods_per_year_for(class: AssetClass, interval: &str, session_hours: f64) -> f64 {
    let days = match class {
        AssetClass::Crypto => 365.0,
        AssetClass::Fx => 260.0,
        AssetClass::Stock | AssetClass::Index => 252.0,
    };
    let hours = match class {
        AssetClass::Crypto | AssetClass::Fx => 24.0,
        AssetClass::Stock | AssetClass::Index => session_hours,
    };
    let per_day = |minutes: f64| hours * 60.0 / minutes;
    match interval {
        "1wk" => 52.0,
        "1mo" => 12.0,
        "1h" | "60m" => days * per_day(60.0),
        "30m" => days * per_day(30.0),
        "15m" => days * per_day(15.0),
        "5m" => days * per_day(5.0),
        _ => days,
    }
}

/// Sharpe Ratio (annualized with 252 bars a year; see [`sharpe_ratio_with`])
pub fn sharpe_ratio(prices: &[f64], risk_free_annual: f64) -> Option<f64> {
    sharpe_ratio_with(prices, risk_free_annual, 252.0)
}

/// Sharpe Ratio annualized for prices sampled `periods_per_year` times a year.
pub fn sharpe_ratio_with(
    prices: &[f64],
    risk_free_annual: f64,
    periods_per_year: f64,
) -> Option<f64> {
    let returns = daily_returns(prices);
    if returns.len() < 2 {
        return None;
    }
    let mean_daily = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance = returns
        .iter()
        .map(|r| (r - mean_daily).powi(2))
        .sum::<f64>()
        / (returns.len() - 1) as f64;
    let std_daily = variance.sqrt();
    if std_daily == 0.0 {
        return None;
    }
    let annual_return = mean_daily * periods_per_year;
    let annual_std = std_daily * periods_per_year.sqrt();
    Some((annual_return - risk_free_annual) / annual_std)
}

/// Sortino Ratio (only penalizes downside volatility; 252 bars a year)
pub fn sortino_ratio(prices: &[f64], risk_free_annual: f64) -> Option<f64> {
    sortino_ratio_with(prices, risk_free_annual, 252.0)
}

/// Sortino Ratio annualized for prices sampled `periods_per_year` times a year.
///
/// The target (minimum acceptable return) is the risk-free rate per period,
/// `risk_free_annual / periods_per_year`. Downside deviation is the root of the
/// mean squared shortfall below that target taken over *all* periods, not only
/// the periods that fell short, so a series with few down periods is not
/// penalised as if every period were a bad one. `None` when no period falls
/// below the target.
pub fn sortino_ratio_with(
    prices: &[f64],
    risk_free_annual: f64,
    periods_per_year: f64,
) -> Option<f64> {
    let returns = daily_returns(prices);
    if returns.len() < 2 || periods_per_year <= 0.0 {
        return None;
    }
    let n = returns.len() as f64;
    let mean_daily = returns.iter().sum::<f64>() / n;
    let target = risk_free_annual / periods_per_year;
    let shortfall_sq: f64 = returns.iter().map(|r| (r - target).min(0.0).powi(2)).sum();
    if shortfall_sq == 0.0 {
        return None;
    }
    let downside_std = (shortfall_sq / n).sqrt() * periods_per_year.sqrt();
    let annual_return = mean_daily * periods_per_year;
    let ratio = (annual_return - risk_free_annual) / downside_std;
    ratio.is_finite().then_some(ratio)
}

/// Maximum drawdown from peak, as a fraction of the peak. A drawdown is only
/// measured against a positive peak, so leading zero or negative prices are
/// skipped until the series first rises above zero. `None` when the series is
/// too short or never has a positive price. A fall to zero is a 100% drawdown.
pub fn max_drawdown(prices: &[f64]) -> Option<(f64, usize, usize)> {
    if prices.len() < 2 {
        return None;
    }
    let mut peak = f64::NEG_INFINITY;
    let mut max_dd = 0.0_f64;
    let mut peak_idx = 0;
    let mut trough_idx = 0;
    let mut current_peak_idx = 0;

    for (i, &price) in prices.iter().enumerate() {
        if price > peak {
            peak = price;
            current_peak_idx = i;
        }
        if peak <= 0.0 {
            continue;
        }
        let dd = (peak - price) / peak;
        if dd > max_dd {
            max_dd = dd;
            peak_idx = current_peak_idx;
            trough_idx = i;
        }
    }
    (peak > 0.0).then_some((max_dd, peak_idx, trough_idx))
}

/// Value at Risk (historical, percentile-based)
pub fn value_at_risk(prices: &[f64], confidence: f64) -> Option<f64> {
    let mut returns = daily_returns(prices);
    if returns.len() < 10 {
        return None;
    }
    returns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((1.0 - confidence) * returns.len() as f64).floor() as usize;
    Some(returns[idx.min(returns.len() - 1)])
}

/// Calmar Ratio (annualized return / max drawdown; 252 bars a year)
pub fn calmar_ratio(prices: &[f64]) -> Option<f64> {
    calmar_ratio_with(prices, 252.0)
}

/// Calmar Ratio for prices sampled `periods_per_year` times a year. The
/// return is annualized over the number of *returns* (one fewer than prices),
/// so a series spanning exactly one year annualizes to its own return.
pub fn calmar_ratio_with(prices: &[f64], periods_per_year: f64) -> Option<f64> {
    if prices.len() < 2 || periods_per_year <= 0.0 {
        return None;
    }
    // A ratio of prices needs a positive start; a series that ends at or
    // below zero (total loss) has no annualised return to speak of.
    let (first, last) = (*prices.first()?, *prices.last()?);
    if !(first > 0.0 && last > 0.0 && first.is_finite() && last.is_finite()) {
        return None;
    }
    let total_return = (last / first) - 1.0;
    let periods = (prices.len() - 1) as f64;
    let annual_return = (1.0 + total_return).powf(periods_per_year / periods) - 1.0;
    let (mdd, _, _) = max_drawdown(prices)?;
    if mdd == 0.0 {
        return None;
    }
    let ratio = annual_return / mdd;
    ratio.is_finite().then_some(ratio)
}

/// Fewest common returns [`dated_correlation`] will compare.
pub const MIN_COMMON_RETURNS: usize = 20;

/// Correlation of the daily returns two dated price series share. Only dates
/// both series traded are used and each return runs between consecutive
/// common dates, so different holiday calendars (NSE and NYSE) or a seven-day
/// crypto week line up on the same days. `None` below
/// [`MIN_COMMON_RETURNS`] returns, instead of a number built from too little.
/// Both series must be in ascending date order.
pub fn dated_correlation(a: &[(NaiveDate, f64)], b: &[(NaiveDate, f64)]) -> Option<f64> {
    let (mut i, mut j) = (0, 0);
    let (mut pa, mut pb) = (Vec::new(), Vec::new());
    while i < a.len() && j < b.len() {
        match a[i].0.cmp(&b[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                pa.push(a[i].1);
                pb.push(b[j].1);
                i += 1;
                j += 1;
            }
        }
    }
    let (ra, rb) = (daily_returns(&pa), daily_returns(&pb));
    // `daily_returns` skips a zero price, which would shift the pairs; only
    // trust the result when both lists kept every return.
    if ra.len() != rb.len() || ra.len() < MIN_COMMON_RETURNS {
        return None;
    }
    pearson(&ra, &rb)
}

/// Pearson correlation between two price series, pairing their returns from
/// the end by position. Use [`dated_correlation`] for series fetched
/// separately: they carry different dates.
pub fn correlation(a: &[f64], b: &[f64]) -> Option<f64> {
    let ra = daily_returns(a);
    let rb = daily_returns(b);
    let n = ra.len().min(rb.len());
    if n < 2 {
        return None;
    }
    pearson(&ra[ra.len() - n..], &rb[rb.len() - n..])
}

/// Pearson correlation of two equal-length series; `None` for fewer than two
/// points or a series that does not vary.
fn pearson(ra: &[f64], rb: &[f64]) -> Option<f64> {
    let n = ra.len();
    if n < 2 || rb.len() != n {
        return None;
    }

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
    if lookback == 0 || highs.len() < lookback || lows.len() < lookback {
        return None;
    }
    let n = highs.len();
    let recent_highs = &highs[n - lookback..];
    let recent_lows = &lows[n - lookback..];
    let swing_high = recent_highs
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let swing_low = recent_lows.iter().cloned().fold(f64::INFINITY, f64::min);
    Some((swing_high, swing_low))
}

// ── Volume Analysis ──

/// On-Balance Volume
pub fn obv(closes: &[f64], volumes: &[u64]) -> Vec<f64> {
    let n = closes.len().min(volumes.len());
    let mut result = Vec::with_capacity(n);
    if n == 0 {
        return result;
    }
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
    let n = closes
        .len()
        .min(highs.len())
        .min(lows.len())
        .min(volumes.len());
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
pub enum GapType {
    Up,
    Down,
}

/// Detect price gaps between consecutive candles
pub fn detect_gaps(opens: &[f64], highs: &[f64], lows: &[f64], closes: &[f64]) -> Vec<Gap> {
    let n = opens
        .len()
        .min(highs.len())
        .min(lows.len())
        .min(closes.len());
    let mut gaps = Vec::new();
    for i in 1..n {
        // Gap up: today's low > yesterday's high
        if lows[i] > highs[i - 1] {
            let gap_low = highs[i - 1];
            let gap_high = lows[i];
            // Check if gap was filled in subsequent candles
            let filled = (i + 1..n).any(|j| lows[j] <= gap_low);
            gaps.push(Gap {
                index: i,
                gap_type: GapType::Up,
                gap_low,
                gap_high,
                filled,
            });
        }
        // Gap down: today's high < yesterday's low
        if highs[i] < lows[i - 1] {
            let gap_low = highs[i];
            let gap_high = lows[i - 1];
            let filled = (i + 1..n).any(|j| highs[j] >= gap_high);
            gaps.push(Gap {
                index: i,
                gap_type: GapType::Down,
                gap_low,
                gap_high,
                filled,
            });
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

/// Position size (whole shares) for a given capital, risk percentage, entry price, and stop price.
///
/// Formula: `floor((capital * (risk_pct / 100.0)) / (entry - stop))`.
/// Returns `0` when `capital <= 0.0`, `risk_pct <= 0.0`, `entry <= 0.0`, or `stop >= entry`.
pub fn shares_for_risk(capital: f64, risk_pct: f64, entry: f64, stop: f64) -> u64 {
    if !(capital > 0.0 && risk_pct > 0.0 && entry > 0.0 && stop < entry) {
        return 0;
    }
    let risk_amount = capital * (risk_pct / 100.0);
    let risk_per_share = entry - stop;
    if risk_per_share <= 0.0 || !risk_per_share.is_finite() {
        return 0;
    }
    let raw = (risk_amount / risk_per_share).floor();
    if raw.is_finite() && raw >= 0.0 {
        raw as u64
    } else {
        0
    }
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
    fn test_shares_for_risk() {
        // Capital = 500,000, risk = 1%, entry = 1,280, stop = 1,231 (risk/share = 49)
        // Risk amount = 5,000 -> 5000 / 49 = 102.04 -> 102 shares
        assert_eq!(shares_for_risk(500_000.0, 1.0, 1280.0, 1231.0), 102);
        // Stop >= entry -> 0 shares
        assert_eq!(shares_for_risk(500_000.0, 1.0, 1280.0, 1280.0), 0);
        assert_eq!(shares_for_risk(500_000.0, 1.0, 1280.0, 1300.0), 0);
        // Zero/negative capital or risk -> 0 shares
        assert_eq!(shares_for_risk(0.0, 1.0, 1280.0, 1231.0), 0);
        assert_eq!(shares_for_risk(-100.0, 1.0, 1280.0, 1231.0), 0);
        assert_eq!(shares_for_risk(500_000.0, 0.0, 1280.0, 1231.0), 0);
    }

    #[test]
    fn test_ema() {
        let data = vec![10.0, 11.0, 12.0, 13.0, 14.0];
        let result = ema(&data, 3);
        assert!(result.is_some());
        // EMA(3) on [10,11,12,13,14]: SMA start=11, then apply EMA formula
        let r = result.unwrap();
        assert!(
            r > 12.0 && r < 15.0,
            "EMA should be between 12 and 15, got {}",
            r
        );
        assert_eq!(ema(&[1.0], 3), None);
    }

    #[test]
    fn test_rsi() {
        // Monotonically increasing = RSI 100
        let up = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
        ];
        assert_eq!(rsi(&up, 14), Some(100.0));
        // Too short
        assert_eq!(rsi(&[1.0, 2.0], 14), None);
    }

    #[test]
    fn test_rsi_range() {
        let data: Vec<f64> = (0..100)
            .map(|i| 100.0 + (i as f64 * 0.1).sin() * 10.0)
            .collect();
        let r = rsi(&data, 14).unwrap();
        assert!((0.0..=100.0).contains(&r));
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
        let prices: Vec<f64> = (0..100)
            .map(|i| 100.0 + (i as f64 * 0.3).sin() * 5.0)
            .collect();
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
        assert!(
            (corr - (-1.0)).abs() < 0.01,
            "Expected -1.0 correlation, got {}",
            corr
        );
    }

    fn dated(start: &str, days: impl IntoIterator<Item = (i64, f64)>) -> Vec<(NaiveDate, f64)> {
        let start = NaiveDate::parse_from_str(start, "%Y-%m-%d").unwrap();
        days.into_iter()
            .map(|(d, p)| (start + chrono::Duration::days(d), p))
            .collect()
    }

    /// A price path with returns that vary from day to day.
    fn path(n: usize) -> Vec<f64> {
        let mut price = 100.0;
        (0..n)
            .map(|i| {
                price *= 1.0 + ((i * 7 % 11) as f64 - 5.0) / 200.0;
                price
            })
            .collect()
    }

    #[test]
    fn an_extra_date_in_one_series_does_not_shift_the_pairs() {
        // B is A's returns on the same days; A also has one extra date in the
        // middle. By position the pairs would drift by a day after it; by
        // date the correlation is exactly 1.
        let p = path(40);
        let b = dated(
            "2026-01-01",
            p.iter().enumerate().map(|(i, x)| (i as i64 * 2, *x)),
        );
        let mut a = b.clone();
        a.insert(20, (b[19].0 + chrono::Duration::days(1), b[19].1 * 1.07));
        let c = dated_correlation(&a, &b).unwrap();
        assert!((c - 1.0).abs() < 1e-9, "{c}");
        // The old positional pairing of the same data is far from 1.
        let pa: Vec<f64> = a.iter().map(|x| x.1).collect();
        let pb: Vec<f64> = b.iter().map(|x| x.1).collect();
        assert!(correlation(&pa, &pb).unwrap() < 0.9);
    }

    #[test]
    fn a_seven_day_week_correlates_with_a_five_day_week_on_common_dates() {
        // Stock B trades Mon-Fri, "crypto" A every day. On the days both
        // trade, A's price is B's price, so the correlation is 1; A's
        // weekend prices must not count.
        let weekday = |d: i64| (d % 7) < 5;
        let p = path(60);
        let b = dated(
            "2026-01-05", // a Monday
            (0..60).filter(|d| weekday(*d)).map(|d| (d, p[d as usize])),
        );
        let a = dated("2026-01-05", (0..60).map(|d| (d, p[d as usize])));
        let c = dated_correlation(&a, &b).unwrap();
        assert!((c - 1.0).abs() < 1e-9, "{c}");
    }

    #[test]
    fn too_few_shared_dates_give_no_correlation() {
        let a = dated("2026-01-01", (0..30).map(|d| (d, 100.0 + d as f64)));
        let b = dated("2026-03-01", (0..30).map(|d| (d, 100.0 + d as f64)));
        assert_eq!(dated_correlation(&a, &b), None);
        // 10 shared dates is under the minimum of 20 returns.
        let c = dated("2026-01-21", (0..30).map(|d| (d, 100.0 + (d * d) as f64)));
        assert_eq!(dated_correlation(&a, &c), None);
        assert_eq!(dated_correlation(&a, &[]), None);
    }

    #[test]
    fn bars_per_year_follow_the_asset_class() {
        use crate::market::AssetClass::*;
        // Daily: 252 sessions, 365 days for crypto, 260 weekdays for FX.
        assert_eq!(periods_per_year_for(Stock, "1d", 6.25), 252.0);
        assert_eq!(periods_per_year_for(Index, "1d", 6.25), 252.0);
        assert_eq!(periods_per_year_for(Crypto, "1d", 6.25), 365.0);
        assert_eq!(periods_per_year_for(Fx, "1d", 6.25), 260.0);
        // 5-minute bars: 75 a NSE session, 78 a US session, 288 a crypto day.
        assert_eq!(periods_per_year_for(Stock, "5m", 6.25), 252.0 * 75.0);
        assert_eq!(periods_per_year_for(Stock, "5m", 6.5), 252.0 * 78.0);
        assert_eq!(periods_per_year_for(Crypto, "5m", 6.25), 365.0 * 288.0);
        assert_eq!(periods_per_year_for(Stock, "1wk", 6.25), 52.0);
        // The old stock-only function is the NSE case.
        assert_eq!(periods_per_year("15m"), 252.0 * 25.0);
    }

    #[test]
    fn crypto_volatility_and_sharpe_use_365_bars() {
        // Daily returns alternating +1% / -1%: the same standard deviation, so
        // annualized volatility scales with sqrt(bars per year).
        let mut prices = vec![100.0];
        for i in 0..60 {
            let last = *prices.last().unwrap();
            prices.push(last * if i % 2 == 0 { 1.01 } else { 0.99 });
        }
        let stock = annualized_volatility_with(&prices, 252.0).unwrap();
        let crypto = annualized_volatility_with(&prices, 365.0).unwrap();
        assert!((crypto / stock - (365.0_f64 / 252.0).sqrt()).abs() < 1e-12);
        // Sharpe with a zero risk-free rate: mean * ppy / (sd * sqrt(ppy)) also
        // scales with sqrt(ppy).
        let s252 = sharpe_ratio_with(&prices, 0.0, 252.0).unwrap();
        let s365 = sharpe_ratio_with(&prices, 0.0, 365.0).unwrap();
        assert!((s365 / s252 - (365.0_f64 / 252.0).sqrt()).abs() < 1e-9);
        // The plain functions are the 252-bar case.
        assert_eq!(
            sharpe_ratio(&prices, 0.05),
            sharpe_ratio_with(&prices, 0.05, 252.0)
        );
    }

    #[test]
    fn calmar_annualizes_over_the_number_of_returns() {
        // 253 prices are 252 returns: one year of daily bars. Doubling in
        // that time is +100% a year, and with a 20% max drawdown Calmar = 5.
        let mut prices: Vec<f64> = (0..253)
            .map(|i| 100.0 * 2.0_f64.powf(i as f64 / 252.0))
            .collect();
        // Insert a 20% dip and recovery without changing the endpoints.
        let peak = prices[100];
        prices[101] = peak * 0.8;
        let (mdd, _, _) = max_drawdown(&prices).unwrap();
        let calmar = calmar_ratio(&prices).unwrap();
        assert!(
            (calmar - 1.0 / mdd).abs() < 1e-9,
            "{calmar} vs {}",
            1.0 / mdd
        );
        // The old exponent used 253 bars and gave a little under 100%.
        assert!(calmar > 1.0 / mdd - 1e-6);
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

    #[test]
    fn test_vwap_basic() {
        let highs = vec![110.0, 112.0, 115.0];
        let lows = vec![90.0, 92.0, 95.0];
        let closes = vec![100.0, 102.0, 105.0];
        let volumes = vec![1000u64, 1200, 1500];
        let result = vwap(&highs, &lows, &closes, &volumes);
        assert!(result.is_some());
        let v = result.unwrap();
        assert!(v > 90.0 && v < 120.0);
    }

    #[test]
    fn test_vwap_zero_volume() {
        let highs = vec![110.0];
        let lows = vec![90.0];
        let closes = vec![100.0];
        let volumes = vec![0u64];
        let result = vwap(&highs, &lows, &closes, &volumes);
        assert!(result.is_none()); // zero total volume → None
    }

    #[test]
    fn test_rsi_signal() {
        assert!(rsi_signal(25.0).to_lowercase().contains("oversold"));
        assert!(rsi_signal(75.0).to_lowercase().contains("overbought"));
        assert_eq!(rsi_signal(50.0), "Neutral");
    }

    #[test]
    fn rsi_and_macd_signals_are_neutral_wording() {
        for rsi in [0.0, 10.0, 25.0, 35.0, 50.0, 65.0, 75.0, 85.0, 100.0] {
            crate::verdict::testing::assert_neutral(rsi_signal(rsi));
        }
        assert_eq!(rsi_signal(75.0), "RSI above 70 (overbought zone)");
        assert_eq!(rsi_signal(25.0), "RSI below 30 (oversold zone)");
        crate::verdict::testing::assert_neutral(macd_signal(1.0));
        crate::verdict::testing::assert_neutral(macd_signal(-1.0));
    }

    #[test]
    fn test_macd_signal() {
        let bull = macd_signal(1.0);
        let bear = macd_signal(-1.0);
        assert!(bull.to_lowercase().contains("bullish"));
        assert!(bear.to_lowercase().contains("bearish"));
    }

    #[test]
    fn test_sortino_ratio() {
        // Use a volatile series so there are downside returns
        let prices: Vec<f64> = (0..100)
            .map(|i| 100.0 + (i as f64 * 0.3).sin() * 10.0)
            .collect();
        let s = sortino_ratio(&prices, 0.0);
        // May be None if no downside deviation, so just verify it doesn't panic
        if let Some(s_val) = s {
            assert!(s_val.is_finite());
        }
    }

    // Prices 100, 110, 99, 108.9, 119.79 give returns +10%, -10%, +10%, +10%.
    // N = 4, mean = 5%, one shortfall of -10% (target 0), so
    // downside deviation = sqrt(0.10^2 / 4) = 5% per period. The old divisor
    // (count of down periods, 1) gave 10% and halved the ratio.
    const SORTINO_PRICES: [f64; 5] = [100.0, 110.0, 99.0, 108.9, 119.79];

    #[test]
    fn sortino_divides_by_all_periods_not_down_periods() {
        // One period a year: ratio = (0.05 - 0) / 0.05 = 1.
        let s = sortino_ratio_with(&SORTINO_PRICES, 0.0, 1.0).unwrap();
        assert!((s - 1.0).abs() < 1e-9, "got {s}");
        // 252 periods a year: both numerator and deviation scale, ratio is
        // 0.05 * 252 / (0.05 * sqrt(252)) = sqrt(252).
        let s = sortino_ratio(&SORTINO_PRICES, 0.0).unwrap();
        assert!((s - 252.0_f64.sqrt()).abs() < 1e-9, "got {s}");
    }

    #[test]
    fn sortino_rarely_down_series_is_not_over_penalised() {
        // 1 down period of -1% among 100 (the other 99 are flat +1%), N = 100.
        // Downside deviation = sqrt(0.01^2 / 100) = 0.001, mean = 0.0098,
        // ratio (1 period a year) = 0.0098 / 0.001 = 9.8. Dividing by the
        // single down period would give 0.0098 / 0.01 = 0.98.
        let mut prices = vec![100.0];
        for i in 0..100 {
            let r = if i == 50 { -0.01 } else { 0.01 };
            let last = *prices.last().unwrap();
            prices.push(last * (1.0 + r));
        }
        let s = sortino_ratio_with(&prices, 0.0, 1.0).unwrap();
        assert!((s - 9.8).abs() < 1e-6, "got {s}");
    }

    #[test]
    fn sortino_measures_shortfall_below_the_risk_free_target() {
        // 4 periods a year, 4% risk-free: target = 1% per period, so the
        // shortfalls are 0 for +10% periods and -11% for the -10% period.
        // Deviation = sqrt(0.11^2 / 4) = 5.5% per period = 11% a year.
        // Annual return = 0.05 * 4 = 20%; ratio = (0.20 - 0.04) / 0.11 = 16/11.
        let s = sortino_ratio_with(&SORTINO_PRICES, 0.04, 4.0).unwrap();
        assert!((s - 16.0 / 11.0).abs() < 1e-9, "got {s}");
    }

    #[test]
    fn sortino_none_without_shortfall_or_data() {
        assert_eq!(sortino_ratio(&[100.0, 101.0, 102.0, 103.0], 0.0), None);
        assert_eq!(sortino_ratio(&[100.0, 99.0], 0.0), None);
        assert_eq!(sortino_ratio_with(&SORTINO_PRICES, 0.0, 0.0), None);
    }

    #[test]
    fn test_calmar_ratio() {
        let prices = vec![100.0, 110.0, 120.0, 100.0, 130.0];
        let cr = calmar_ratio(&prices);
        // Could be None if max_drawdown is 0 or prices too short
        if let Some(c) = cr {
            assert!(c.is_finite());
        }
    }

    #[test]
    fn max_drawdown_by_hand_and_with_bad_prices() {
        // Peak 120 at index 2, trough 90 at index 3: (120 - 90) / 120 = 0.25.
        let (dd, p, t) = max_drawdown(&[100.0, 110.0, 120.0, 90.0, 95.0]).unwrap();
        assert!((dd - 0.25).abs() < 1e-12 && (p, t) == (2, 3));
        // Falling to zero is a 100% drawdown from the 50 peak.
        assert_eq!(max_drawdown(&[50.0, 40.0, 0.0]), Some((1.0, 0, 2)));
        // No positive price, so no peak to measure from: None, never NaN/inf.
        assert_eq!(max_drawdown(&[0.0, 0.0, 0.0]), None);
        assert_eq!(max_drawdown(&[-5.0, -3.0]), None);
        // Leading zero is skipped; the drawdown is measured from the first
        // positive peak (10 down to 5 = 0.5).
        let (dd, p, t) = max_drawdown(&[0.0, 10.0, 5.0]).unwrap();
        assert!((dd - 0.5).abs() < 1e-12 && (p, t) == (1, 2));
        // NaN prices do not poison the result.
        let (dd, _, _) = max_drawdown(&[100.0, f64::NAN, 80.0]).unwrap();
        assert!((dd - 0.2).abs() < 1e-12);
    }

    #[test]
    fn calmar_ratio_by_hand() {
        // 100 -> 121 is 3 returns; at 3 periods a year that is exactly one
        // year, so the annual return is the total 21%. The deepest drawdown
        // is 110 -> 105 = 5/110, so the ratio is 0.21 / (5/110) = 4.62.
        let c = calmar_ratio_with(&[100.0, 110.0, 105.0, 121.0], 3.0).unwrap();
        assert!((c - 0.21 / (5.0 / 110.0)).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn calmar_ratio_none_for_zero_or_negative_prices_and_total_loss() {
        // Zero start would divide by zero.
        assert_eq!(calmar_ratio(&[0.0, 10.0, 20.0]), None);
        assert_eq!(calmar_ratio(&[-10.0, 10.0, 20.0]), None);
        // Total loss, and a series that ends below zero, would raise a
        // non-positive base to a fractional power (NaN).
        assert_eq!(calmar_ratio(&[100.0, 50.0, 0.0]), None);
        assert_eq!(calmar_ratio(&[100.0, 50.0, -10.0]), None);
        assert_eq!(calmar_ratio(&[f64::NAN, 1.0, 2.0]), None);
        // Zero periods per year is not a sampling rate.
        assert_eq!(calmar_ratio_with(&[100.0, 90.0, 110.0], 0.0), None);
    }

    #[test]
    fn test_find_swing_points() {
        let highs: Vec<f64> = (0..30)
            .map(|i| 100.0 + (i as f64 * 0.3).sin() * 10.0)
            .collect();
        let lows: Vec<f64> = highs.iter().map(|h| h - 5.0).collect();
        let result = find_swing_points(&highs, &lows, 5);
        assert!(result.is_some());
        let (swing_high, swing_low) = result.unwrap();
        assert!(swing_high >= swing_low);
    }

    #[test]
    fn test_ad_line_basic() {
        let highs = vec![110.0, 115.0, 112.0];
        let lows = vec![90.0, 92.0, 95.0];
        let closes = vec![105.0, 112.0, 108.0];
        let volumes = vec![1000u64, 1200, 900];
        let result = ad_line(&highs, &lows, &closes, &volumes);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_atr_basic() {
        let highs = vec![110.0, 112.0, 115.0, 113.0, 116.0];
        let lows = vec![90.0, 92.0, 95.0, 93.0, 96.0];
        let closes = vec![100.0, 102.0, 105.0, 103.0, 106.0];
        let result = atr(&highs, &lows, &closes, 3);
        assert!(result.is_some());
        assert!(result.unwrap() > 0.0);
    }

    #[test]
    fn test_chandelier_exit() {
        let highs = vec![110.0, 115.0, 112.0, 118.0, 120.0];
        let result = chandelier_exit(&highs, 2.0, 3.0);
        if let Some(exit) = result {
            assert!(exit > 0.0);
            assert!(
                exit <= *highs
                    .iter()
                    .fold(&f64::NEG_INFINITY, |a, b| if b > a { b } else { a })
            );
        }
    }

    #[test]
    fn test_sma_insufficient_data() {
        assert_eq!(sma(&[1.0, 2.0], 5), None);
    }

    #[test]
    fn test_fibonacci_levels_range() {
        let levels = fibonacci_levels(200.0, 100.0);
        for level in &levels {
            assert!(*level >= 100.0 && *level <= 200.0);
        }
    }

    #[test]
    fn test_max_drawdown_no_drawdown() {
        let prices = vec![100.0, 110.0, 120.0, 130.0];
        let result = max_drawdown(&prices);
        if let Some((dd, _, _)) = result {
            assert_eq!(dd, 0.0); // monotonically increasing, no drawdown
        }
    }

    #[test]
    fn test_vwap_mismatched_lengths_no_panic() {
        // Yahoo can return a null in one OHLCV channel but not others, giving
        // ragged vectors. vwap must not index out of bounds.
        let highs = vec![10.0, 11.0, 12.0, 13.0];
        let lows = vec![9.0, 10.0]; // shorter
        let closes = vec![9.5, 10.5, 11.5];
        let volumes = vec![100u64, 200]; // shortest
        let v = vwap(&highs, &lows, &closes, &volumes);
        assert!(v.is_some());
    }

    #[test]
    fn test_atr_mismatched_lengths_no_panic() {
        let highs = vec![10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
        let lows = vec![9.0, 10.0, 11.0]; // shorter than highs
        let closes = vec![9.5, 10.5, 11.5, 12.5];
        // Must return None (not panic) when the common length is below period+1.
        assert!(atr(&highs, &lows, &closes, 14).is_none());
    }

    #[test]
    fn test_daily_returns_skips_zero_price() {
        // A 0.0 price must not produce Inf/NaN — that window is skipped.
        let prices = vec![100.0, 0.0, 50.0, 55.0];
        let returns = daily_returns(&prices);
        assert!(returns.iter().all(|r| r.is_finite()));
    }

    #[test]
    fn percent_change_guards_non_positive_start() {
        assert!((percent_change(100.0, 110.0) - 10.0).abs() < 1e-9);
        assert_eq!(percent_change(0.0, 50.0), 0.0);
        assert_eq!(percent_change(-1.0, 50.0), 0.0);
        assert!(percent_change(0.0, 50.0).is_finite());
        assert!(!percent_change(0.0, 1.0).is_infinite());
    }

    #[test]
    fn test_value_at_risk_no_panic_with_zero_price() {
        // A 0.0 in the series would historically NaN-poison the sort and panic.
        let mut prices: Vec<f64> = (0..20).map(|i| 100.0 + i as f64).collect();
        prices[5] = 0.0;
        let _ = value_at_risk(&prices, 0.95); // must not panic
    }

    #[test]
    fn test_find_swing_points_zero_lookback_is_none() {
        assert!(find_swing_points(&[], &[], 0).is_none());
    }

    #[test]
    fn weekly_volatility_is_annualized_with_52_periods() {
        // Same alternating ±1% moves: as daily bars vs weekly bars.
        let prices: Vec<f64> = (0..60)
            .map(|i| if i % 2 == 0 { 100.0 } else { 101.0 })
            .collect();
        let daily = annualized_volatility(&prices).unwrap();
        let weekly = annualized_volatility_with(&prices, periods_per_year("1wk")).unwrap();
        assert!((daily / weekly - (252.0_f64 / 52.0).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn xirr_of_monthly_sip_matches_known_rate() {
        // 60 monthly instalments of 100 into an asset growing 1% a month.
        let mut flows: Vec<(f64, f64)> = (0..60).map(|i| (i as f64 / 12.0, -100.0)).collect();
        let fv: f64 = (0..60).map(|i| 100.0 * 1.01_f64.powi(59 - i)).sum();
        flows.push((59.0 / 12.0, fv));
        let r = xirr(&flows).unwrap();
        assert!((r - (1.01_f64.powi(12) - 1.0)).abs() < 1e-6, "{r}");
    }

    #[test]
    fn xirr_lump_sum_doubling_in_one_year_is_100_percent() {
        let r = xirr(&[(0.0, -100.0), (1.0, 200.0)]).unwrap();
        assert!((r - 1.0).abs() < 1e-6);
        assert!(xirr(&[(0.0, -100.0)]).is_none());
    }
}
