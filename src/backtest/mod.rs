//! Backtesting engine.
//!
//! - [`rules`]: the strategy format — TOML entry/exit rules over the
//!   indicators in `technical.rs`. The six built-ins are bundled from
//!   `examples/strategies/`.
//! - [`costs`]: Indian (delivery, intraday) and US trading costs, slippage.
//! - [`engine`]: the simulator for one symbol or a portfolio — next-open
//!   fills, stops and targets, rebalancing — and performance metrics.
//! - [`walkforward`]: rolling train/test parameter optimisation.
//! - [`stats`]: Probabilistic and Deflated Sharpe ratios.
//! - [`indicators`]: whole-series indicators matching `technical.rs`.
//! - [`plugin`]: sandboxed WASM strategy plugins (`wasm-plugins` feature).

pub mod costs;
pub mod engine;
pub mod indicators;
pub mod plugin;
#[cfg(test)]
mod regression;
pub mod rules;
pub mod stats;
pub mod walkforward;

use std::ops::Range;

/// Daily OHLCV bars, oldest first. Every bar has all four prices.
#[derive(Debug, Clone, Default)]
pub struct Bars {
    /// Unix seconds of the bar's start.
    pub time: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<u64>,
}

impl Bars {
    pub fn len(&self) -> usize {
        self.close.len()
    }

    pub fn is_empty(&self) -> bool {
        self.close.is_empty()
    }

    /// Bars from a Yahoo chart. A bar missing any price (a halt, or Yahoo's
    /// half-filled live bar) is dropped rather than guessed; missing volume
    /// counts as zero. When two bars share a UTC day the later one wins.
    pub fn from_chart(chart: &crate::api::ChartData) -> Bars {
        let mut bars = Bars::default();
        let Some(q) = chart.indicators.quote.first() else {
            return bars;
        };
        let times = chart.timestamp.as_deref().unwrap_or_default();
        let at = |s: &Option<Vec<Option<f64>>>, i: usize| {
            s.as_ref()
                .and_then(|v| v.get(i).copied().flatten())
                .filter(|p| p.is_finite() && *p > 0.0)
        };
        for (i, &t) in times.iter().enumerate() {
            let (Some(o), Some(h), Some(l), Some(c)) = (
                at(&q.open, i),
                at(&q.high, i),
                at(&q.low, i),
                at(&q.close, i),
            ) else {
                continue;
            };
            let v = q
                .volume
                .as_ref()
                .and_then(|v| v.get(i).copied().flatten())
                .unwrap_or(0);
            if bars.time.last().is_some_and(|&p| day(p) == day(t)) {
                bars.pop();
            }
            bars.push(t, o, h.max(o).max(c), l.min(o).min(c), c, v);
        }
        bars
    }

    /// Bars whose prices include dividends: each bar's open, high, low and
    /// close are scaled by its `adjclose / close`, so a strategy's returns
    /// count the dividends a holder received. Charts without Yahoo's
    /// `adjclose` (other sources, intraday bars) give [`Bars::from_chart`].
    pub fn from_chart_total_return(chart: &crate::api::ChartData) -> Bars {
        let adj = chart.return_close_series();
        let close = chart
            .indicators
            .quote
            .first()
            .and_then(|q| q.close.as_deref())
            .unwrap_or_default();
        if !chart.has_adjclose() || adj.len() != close.len() {
            return Bars::from_chart(chart);
        }
        let factors: Vec<Option<f64>> = adj
            .iter()
            .zip(close)
            .map(|(a, c)| match (a, c) {
                (Some(a), Some(c)) if *c > 0.0 && *a > 0.0 => Some(a / c),
                _ => None,
            })
            .collect();
        let mut scaled = crate::api::ChartData {
            meta: None,
            timestamp: chart.timestamp.clone(),
            indicators: crate::api::Indicators::default(),
            events: None,
        };
        let Some(q) = chart.indicators.quote.first() else {
            return Bars::default();
        };
        let scale = |s: &Option<Vec<Option<f64>>>| {
            s.as_ref().map(|v| {
                v.iter()
                    .enumerate()
                    .map(|(i, p)| Some((*p)? * factors.get(i).copied().flatten()?))
                    .collect()
            })
        };
        scaled.indicators.quote.push(crate::api::QuoteIndicator {
            open: scale(&q.open),
            high: scale(&q.high),
            low: scale(&q.low),
            close: scale(&q.close),
            volume: q.volume.clone(),
        });
        Bars::from_chart(&scaled)
    }

    fn push(&mut self, t: i64, o: f64, h: f64, l: f64, c: f64, v: u64) {
        self.time.push(t);
        self.open.push(o);
        self.high.push(h);
        self.low.push(l);
        self.close.push(c);
        self.volume.push(v);
    }

    fn pop(&mut self) {
        self.time.pop();
        self.open.pop();
        self.high.pop();
        self.low.pop();
        self.close.pop();
        self.volume.pop();
    }

    pub fn slice(&self, r: Range<usize>) -> Bars {
        Bars {
            time: self.time[r.clone()].to_vec(),
            open: self.open[r.clone()].to_vec(),
            high: self.high[r.clone()].to_vec(),
            low: self.low[r.clone()].to_vec(),
            close: self.close[r.clone()].to_vec(),
            volume: self.volume[r].to_vec(),
        }
    }

    /// Bars where open = high = low = close, one per day from 2024-01-01.
    #[cfg(test)]
    pub fn from_closes(closes: &[f64]) -> Bars {
        let mut b = Bars::default();
        for (i, &c) in closes.iter().enumerate() {
            b.push(1_704_067_200 + i as i64 * 86_400, c, c, c, c, 1_000);
        }
        b
    }
}

/// Days since the Unix epoch (UTC) — the calendar key for aligning symbols.
pub fn day(t: i64) -> i64 {
    t.div_euclid(86_400)
}

/// `YYYY-MM-DD` (UTC) for a bar time.
pub fn date(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Price return over the bars, in percent.
pub fn buy_and_hold_pct(bars: &Bars) -> Option<f64> {
    let (first, last) = (bars.close.first()?, bars.close.last()?);
    Some((last / first - 1.0) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ChartData, Indicators, QuoteIndicator};

    #[test]
    fn total_return_bars_scale_prices_by_adjclose() {
        let chart = ChartData {
            meta: None,
            events: None,
            timestamp: Some(vec![0, 86_400]),
            indicators: Indicators {
                quote: vec![QuoteIndicator {
                    open: Some(vec![Some(100.0), Some(100.0)]),
                    high: Some(vec![Some(100.0), Some(100.0)]),
                    low: Some(vec![Some(100.0), Some(100.0)]),
                    close: Some(vec![Some(100.0), Some(100.0)]),
                    volume: Some(vec![Some(1), Some(1)]),
                }],
                adjclose: Some(vec![crate::api::AdjCloseIndicator {
                    adjclose: Some(vec![Some(98.0), Some(100.0)]),
                }]),
            },
        };
        let b = Bars::from_chart_total_return(&chart);
        assert_eq!(b.close, vec![98.0, 100.0]);
        assert_eq!(b.open, vec![98.0, 100.0]);
        // Without adjclose the raw prices stand.
        let mut raw = chart;
        raw.indicators.adjclose = None;
        assert_eq!(Bars::from_chart_total_return(&raw).close, vec![100.0, 100.0]);
    }

    #[test]
    fn from_chart_drops_incomplete_bars_and_same_day_duplicates() {
        let chart = ChartData {
            meta: None,
            events: None,
            timestamp: Some(vec![0, 86_400, 2 * 86_400, 2 * 86_400 + 3_600]),
            indicators: Indicators {
                quote: vec![QuoteIndicator {
                    open: Some(vec![Some(10.0), None, Some(12.0), Some(12.5)]),
                    high: Some(vec![Some(11.0), Some(11.0), Some(13.0), Some(13.5)]),
                    low: Some(vec![Some(9.0), Some(9.0), Some(11.0), Some(11.5)]),
                    close: Some(vec![Some(10.5), Some(10.0), Some(12.5), Some(13.0)]),
                    volume: Some(vec![Some(100), Some(100), None, Some(300)]),
                }],
                adjclose: None,
            },
        };
        let b = Bars::from_chart(&chart);
        assert_eq!(b.len(), 2); // day 1 has no open; day 2's later bar wins
        assert_eq!(b.close, vec![10.5, 13.0]);
        assert_eq!(b.volume, vec![100, 300]);
        assert_eq!(date(b.time[1]), "1970-01-03");
    }

    #[test]
    fn buy_and_hold() {
        let b = Bars::from_closes(&[100.0, 120.0, 150.0]);
        assert!((buy_and_hold_pct(&b).unwrap() - 50.0).abs() < 1e-9);
        assert!(buy_and_hold_pct(&Bars::default()).is_none());
    }
}
