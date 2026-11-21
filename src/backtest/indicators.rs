//! Whole-series versions of the indicators in `technical.rs`.
//!
//! `technical::rsi(&closes[..=i], 14)` recomputes from the first bar for every
//! `i`; a backtest needs the value at every bar, so these compute the whole
//! series in one pass. Entry `i` is the indicator as of bar `i` (using bars
//! `0..=i` only — no look-ahead), or `None` until there is enough history.
//! The tests check every value against `technical.rs`.

pub type Series = Vec<Option<f64>>;

pub fn sma(data: &[f64], period: usize) -> Series {
    let mut out = vec![None; data.len()];
    if period == 0 {
        return out;
    }
    for i in (period - 1)..data.len() {
        out[i] = Some(data[i + 1 - period..=i].iter().sum::<f64>() / period as f64);
    }
    out
}

/// EMA seeded with the SMA of the first `period` values, as in `technical::ema`.
pub fn ema(data: &[f64], period: usize) -> Series {
    let mut out = vec![None; data.len()];
    if period == 0 || data.len() < period {
        return out;
    }
    let k = 2.0 / (period as f64 + 1.0);
    let mut v = data[..period].iter().sum::<f64>() / period as f64;
    out[period - 1] = Some(v);
    for i in period..data.len() {
        v = (data[i] - v) * k + v;
        out[i] = Some(v);
    }
    out
}

/// EMA of a series that starts with `None`s (the MACD signal line).
fn ema_of(series: &Series, period: usize) -> Series {
    let start = series.iter().position(Option::is_some);
    let mut out = vec![None; series.len()];
    if let Some(start) = start {
        let values: Vec<f64> = series[start..].iter().map(|v| v.unwrap_or(0.0)).collect();
        for (j, v) in ema(&values, period).into_iter().enumerate() {
            out[start + j] = v;
        }
    }
    out
}

/// Wilder-smoothed RSI, as in `technical::rsi`.
pub fn rsi(data: &[f64], period: usize) -> Series {
    let mut out = vec![None; data.len()];
    if period == 0 || data.len() < period + 1 {
        return out;
    }
    let change = |i: usize| data[i] - data[i - 1];
    let gain = |i: usize| change(i).max(0.0);
    let loss = |i: usize| {
        if change(i) > 0.0 {
            0.0
        } else {
            change(i).abs()
        }
    };
    let p = period as f64;
    let mut avg_gain = (1..=period).map(gain).sum::<f64>() / p;
    let mut avg_loss = (1..=period).map(loss).sum::<f64>() / p;
    let value = |g: f64, l: f64| {
        if l == 0.0 {
            100.0
        } else {
            100.0 - 100.0 / (1.0 + g / l)
        }
    };
    out[period] = Some(value(avg_gain, avg_loss));
    for i in (period + 1)..data.len() {
        avg_gain = (avg_gain * (p - 1.0) + gain(i)) / p;
        avg_loss = (avg_loss * (p - 1.0) + loss(i)) / p;
        out[i] = Some(value(avg_gain, avg_loss));
    }
    out
}

/// MACD line, signal line and histogram. `technical::macd` is `(12, 26, 9)`.
pub fn macd(data: &[f64], fast: usize, slow: usize, signal: usize) -> (Series, Series, Series) {
    let (f, s) = (ema(data, fast), ema(data, slow));
    let line: Series = f.iter().zip(&s).map(|(a, b)| Some((*a)? - (*b)?)).collect();
    let sig = ema_of(&line, signal);
    let hist = line
        .iter()
        .zip(&sig)
        .map(|(l, s)| Some((*l)? - (*s)?))
        .collect();
    (line, sig, hist)
}

/// Bollinger Bands (upper, middle, lower) with `k` population standard
/// deviations. `technical::bollinger_bands` uses `k = 2`.
pub fn bollinger(data: &[f64], period: usize, k: f64) -> (Series, Series, Series) {
    let n = data.len();
    let (mut up, mut mid, mut lo) = (vec![None; n], vec![None; n], vec![None; n]);
    if period == 0 {
        return (up, mid, lo);
    }
    for i in (period - 1)..n {
        let slice = &data[i + 1 - period..=i];
        let m = slice.iter().sum::<f64>() / period as f64;
        let sd = (slice.iter().map(|x| (x - m).powi(2)).sum::<f64>() / period as f64).sqrt();
        up[i] = Some(m + k * sd);
        mid[i] = Some(m);
        lo[i] = Some(m - k * sd);
    }
    (up, mid, lo)
}

/// Wilder-smoothed average true range, as in `technical::atr`.
pub fn atr(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> Series {
    let n = highs.len().min(lows.len()).min(closes.len());
    let mut out = vec![None; n];
    if period == 0 || n < period + 1 {
        return out;
    }
    let tr = |i: usize| {
        (highs[i] - lows[i])
            .max((highs[i] - closes[i - 1]).abs())
            .max((lows[i] - closes[i - 1]).abs())
    };
    let p = period as f64;
    let mut v = (1..=period).map(tr).sum::<f64>() / p;
    out[period] = Some(v);
    for i in (period + 1)..n {
        v = (v * (p - 1.0) + tr(i)) / p;
        out[i] = Some(v);
    }
    out
}

/// Cumulative VWAP from the first bar, as in `technical::vwap`.
pub fn vwap(highs: &[f64], lows: &[f64], closes: &[f64], volumes: &[u64]) -> Series {
    let n = highs
        .len()
        .min(lows.len())
        .min(closes.len())
        .min(volumes.len());
    let (mut tp_vol, mut vol) = (0.0, 0.0);
    (0..n)
        .map(|i| {
            let tp = (highs[i] + lows[i] + closes[i]) / 3.0;
            tp_vol += tp * volumes[i] as f64;
            vol += volumes[i] as f64;
            (vol != 0.0).then(|| tp_vol / vol)
        })
        .collect()
}

/// Highest value of `data` over the `period` bars before bar `i` (not
/// including bar `i`), so `close > highest(20)` is a 20-day breakout.
pub fn highest(data: &[f64], period: usize) -> Series {
    window_extreme(data, period, f64::max)
}

/// Lowest value of `data` over the `period` bars before bar `i`.
pub fn lowest(data: &[f64], period: usize) -> Series {
    window_extreme(data, period, f64::min)
}

fn window_extreme(data: &[f64], period: usize, pick: fn(f64, f64) -> f64) -> Series {
    (0..data.len())
        .map(|i| {
            (period > 0 && i >= period).then(|| data[i - period..i].iter().copied().reduce(pick))?
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::technical;

    /// A noisy, trending series long enough for every indicator.
    fn prices() -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<u64>) {
        let closes: Vec<f64> = (0..240)
            .map(|i| {
                let t = i as f64;
                100.0 + t * 0.15 + (t * 0.21).sin() * 6.0 + (t * 1.3).cos() * 1.5
            })
            .collect();
        let highs = closes.iter().map(|c| c + 1.2).collect();
        let lows = closes.iter().map(|c| c - 0.9).collect();
        let vols = (0..240).map(|i| 1_000 + (i * 37 % 500) as u64).collect();
        (closes, highs, lows, vols)
    }

    /// Series value at `i` must equal the point-in-time function on `0..=i`.
    fn assert_matches(name: &str, series: &Series, point: impl Fn(usize) -> Option<f64>) {
        for (i, v) in series.iter().enumerate() {
            let expected = point(i);
            match (v, expected) {
                (Some(a), Some(b)) => assert!(
                    (a - b).abs() <= 1e-9 * b.abs().max(1.0),
                    "{name}[{i}]: {a} vs {b}"
                ),
                (None, None) => {}
                _ => panic!("{name}[{i}]: {v:?} vs {expected:?}"),
            }
        }
    }

    #[test]
    fn matches_technical_rs() {
        let (c, h, l, v) = prices();
        assert_matches("sma20", &sma(&c, 20), |i| technical::sma(&c[..=i], 20));
        assert_matches("ema12", &ema(&c, 12), |i| technical::ema(&c[..=i], 12));
        assert_matches("rsi14", &rsi(&c, 14), |i| technical::rsi(&c[..=i], 14));
        assert_matches("atr14", &atr(&h, &l, &c, 14), |i| {
            technical::atr(&h[..=i], &l[..=i], &c[..=i], 14)
        });
        assert_matches("vwap", &vwap(&h, &l, &c, &v), |i| {
            technical::vwap(&h[..=i], &l[..=i], &c[..=i], &v[..=i])
        });
        let (up, mid, lo) = bollinger(&c, 20, 2.0);
        let bb = |i: usize| technical::bollinger_bands(&c[..=i], 20);
        assert_matches("bb.upper", &up, |i| bb(i).map(|b| b.0));
        assert_matches("bb.middle", &mid, |i| bb(i).map(|b| b.1));
        assert_matches("bb.lower", &lo, |i| bb(i).map(|b| b.2));
    }

    #[test]
    fn macd_matches_technical_rs_once_the_signal_line_exists() {
        let (c, ..) = prices();
        let (line, sig, hist) = macd(&c, 12, 26, 9);
        for i in 33..c.len() {
            let (l, s, h) = technical::macd(&c[..=i]).unwrap();
            assert!((line[i].unwrap() - l).abs() < 1e-9, "line[{i}]");
            assert!((sig[i].unwrap() - s).abs() < 1e-9, "signal[{i}]");
            assert!((hist[i].unwrap() - h).abs() < 1e-9, "hist[{i}]");
        }
        assert!(line[24].is_none() && line[25].is_some());
        assert!(sig[32].is_none() && sig[33].is_some());
        assert!(technical::macd(&c[..=32]).is_none());
    }

    #[test]
    fn highest_and_lowest_exclude_the_current_bar() {
        let d = [1.0, 5.0, 3.0, 2.0, 4.0];
        assert_eq!(
            highest(&d, 2),
            vec![None, None, Some(5.0), Some(5.0), Some(3.0)]
        );
        assert_eq!(
            lowest(&d, 2),
            vec![None, None, Some(1.0), Some(3.0), Some(2.0)]
        );
    }

    #[test]
    fn short_inputs_give_none() {
        assert!(rsi(&[1.0, 2.0], 14).iter().all(Option::is_none));
        assert!(sma(&[1.0], 0).iter().all(Option::is_none));
        assert!(ema(&[], 3).is_empty());
    }
}
