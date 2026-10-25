//! Alerts: what `alert add` stores in `alerts.json`, and how a condition is
//! evaluated against a live quote, daily bars or the results calendar.
//!
//! Price alerts (`above` / `below`) compare the live price with a level.
//! Technical alerts read daily bars through the indicators in
//! `technical.rs`; the last bar is the current (possibly still trading)
//! session, so a cross fires on the bar where it happens and not on the
//! days after. `results-in` reads the next results date from `calendar.rs`.
//!
//! `alerts.json` from before technical alerts loads unchanged: the new
//! conditions are new enum names and `period` is optional.

use crate::backtest::Bars;
use crate::i18n::{Lang, Msg, tf_in};
use crate::market::Market;
use crate::technical;
use anyhow::{Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub symbol: String,
    pub condition: AlertCondition,
    /// The condition's number: a price, an RSI level, a moving-average
    /// length, a look-back in days, a percent move, a volume multiple or a
    /// number of days before results (see [`AlertCondition`]).
    pub target: f64,
    /// RSI period or the length of the volume average; only set for the
    /// conditions that take one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<usize>,
    pub created_at: String,
}

/// What an alert watches. Serialized by variant name (`"Above"`, `"RsiBelow"`)
/// as `alerts.json` always has; the CLI spelling is [`AlertCondition::name`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AlertCondition {
    /// Live price at or above `target`.
    Above,
    /// Live price at or below `target`.
    Below,
    /// RSI(`period`) at or below `target`.
    RsiBelow,
    /// RSI(`period`) at or above `target`.
    RsiAbove,
    /// Close crosses above its `target`-day simple moving average.
    CrossAboveSma,
    /// Close crosses below its `target`-day simple moving average.
    CrossBelowSma,
    /// Close crosses above its `target`-day exponential moving average.
    CrossAboveEma,
    /// Close crosses below its `target`-day exponential moving average.
    CrossBelowEma,
    /// Close above the highest high of the previous `target` sessions.
    BreakoutHigh,
    /// Close below the lowest low of the previous `target` sessions.
    BreakdownLow,
    /// Day change at or above `target` percent (positive).
    ChangeAbove,
    /// Day change at or below `target` percent (negative).
    ChangeBelow,
    /// Volume at least `target` times its `period`-day average.
    VolumeSpike,
    /// Results due within `target` days.
    ResultsIn,
}

/// Default RSI period.
pub const RSI_PERIOD: usize = 14;
/// Default length of the average a volume spike is measured against.
pub const VOLUME_PERIOD: usize = 20;

impl AlertCondition {
    pub const ALL: [AlertCondition; 14] = [
        AlertCondition::Above,
        AlertCondition::Below,
        AlertCondition::RsiBelow,
        AlertCondition::RsiAbove,
        AlertCondition::CrossAboveSma,
        AlertCondition::CrossBelowSma,
        AlertCondition::CrossAboveEma,
        AlertCondition::CrossBelowEma,
        AlertCondition::BreakoutHigh,
        AlertCondition::BreakdownLow,
        AlertCondition::ChangeAbove,
        AlertCondition::ChangeBelow,
        AlertCondition::VolumeSpike,
        AlertCondition::ResultsIn,
    ];

    /// The CLI spelling, e.g. `rsi-below`.
    pub fn name(self) -> &'static str {
        match self {
            AlertCondition::Above => "above",
            AlertCondition::Below => "below",
            AlertCondition::RsiBelow => "rsi-below",
            AlertCondition::RsiAbove => "rsi-above",
            AlertCondition::CrossAboveSma => "cross-above-sma",
            AlertCondition::CrossBelowSma => "cross-below-sma",
            AlertCondition::CrossAboveEma => "cross-above-ema",
            AlertCondition::CrossBelowEma => "cross-below-ema",
            AlertCondition::BreakoutHigh => "breakout-high",
            AlertCondition::BreakdownLow => "breakdown-low",
            AlertCondition::ChangeAbove => "change-above",
            AlertCondition::ChangeBelow => "change-below",
            AlertCondition::VolumeSpike => "volume-spike",
            AlertCondition::ResultsIn => "results-in",
        }
    }

    /// Parse a CLI spelling (`>` and `<` mean above and below).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            ">" => Some(AlertCondition::Above),
            "<" => Some(AlertCondition::Below),
            other => Self::ALL.into_iter().find(|c| c.name() == other),
        }
    }

    /// The default period for conditions that take `--period`.
    pub fn default_period(self) -> Option<usize> {
        match self {
            AlertCondition::RsiBelow | AlertCondition::RsiAbove => Some(RSI_PERIOD),
            AlertCondition::VolumeSpike => Some(VOLUME_PERIOD),
            _ => None,
        }
    }

    /// Conditions about one session (today's change or volume): each new
    /// session may notify again, even if the last one ended triggered.
    pub fn per_session(self) -> bool {
        matches!(
            self,
            AlertCondition::ChangeAbove | AlertCondition::ChangeBelow | AlertCondition::VolumeSpike
        )
    }

    /// Price alerts need only the live quote.
    pub fn is_price(self) -> bool {
        matches!(
            self,
            AlertCondition::Above
                | AlertCondition::Below
                | AlertCondition::ChangeAbove
                | AlertCondition::ChangeBelow
        )
    }
}

impl std::fmt::Display for AlertCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// What [`Alert::evaluate`] reads for one symbol. Fields a condition does
/// not need may be left empty.
#[derive(Debug, Clone, Default)]
pub struct MarketData {
    /// Live price.
    pub price: Option<f64>,
    pub currency: Option<String>,
    /// Day change in percent, from the quote.
    pub change_percent: Option<f64>,
    /// Daily bars, oldest first; the last bar is the latest session.
    pub bars: Option<Bars>,
    /// The next results date, when one is known.
    pub next_results: Option<NaiveDate>,
    /// The exchange's current session date (for `results-in`).
    pub today: Option<NaiveDate>,
}

/// The outcome of one evaluation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// The condition holds now.
    pub hit: bool,
    /// The number compared with the target: the price, the RSI, the day
    /// change, the volume multiple or the days until results; the close for
    /// crosses and breakouts.
    pub value: f64,
    /// The moving level compared with: the moving average, the previous
    /// high or low, or the average volume.
    pub reference: Option<f64>,
}

impl Alert {
    /// RSI period or volume-average length, with the default filled in.
    pub fn period(&self) -> Option<usize> {
        self.period.or(self.condition.default_period())
    }

    /// The target as a whole number of days or bars.
    fn length(&self) -> usize {
        self.target.round().max(0.0) as usize
    }

    /// Daily bars the condition needs (0 when it needs none).
    pub fn bars_needed(&self) -> usize {
        match self.condition {
            AlertCondition::RsiBelow | AlertCondition::RsiAbove => {
                // Wilder smoothing settles after a few periods.
                (self.period().unwrap_or(RSI_PERIOD) * 5).max(60)
            }
            AlertCondition::CrossAboveSma | AlertCondition::CrossBelowSma => self.length() + 1,
            // EMAs need warm-up beyond their length to match charting tools.
            AlertCondition::CrossAboveEma | AlertCondition::CrossBelowEma => self.length() * 3 + 1,
            AlertCondition::BreakoutHigh | AlertCondition::BreakdownLow => self.length() + 1,
            AlertCondition::VolumeSpike => self.period().unwrap_or(VOLUME_PERIOD) + 1,
            _ => 0,
        }
    }

    /// Whether the condition looks at the results calendar.
    pub fn needs_results(&self) -> bool {
        self.condition == AlertCondition::ResultsIn
    }

    /// Evaluate against `data`. `None` when the data the condition needs is
    /// missing (no live price, too little history, no results date).
    pub fn evaluate(&self, data: &MarketData) -> Option<Reading> {
        let target = self.target;
        let level = |value: f64, hit: bool| Reading {
            hit,
            value,
            reference: None,
        };
        let bars = || data.bars.as_ref().filter(|b| !b.is_empty());
        match self.condition {
            AlertCondition::Above => data.price.filter(valid).map(|p| level(p, p >= target)),
            AlertCondition::Below => data.price.filter(valid).map(|p| level(p, p <= target)),
            AlertCondition::ChangeAbove => data
                .change_percent
                .filter(|c| c.is_finite())
                .map(|c| level(c, c >= target)),
            AlertCondition::ChangeBelow => data
                .change_percent
                .filter(|c| c.is_finite())
                .map(|c| level(c, c <= target)),
            AlertCondition::RsiBelow | AlertCondition::RsiAbove => {
                let period = self.period().unwrap_or(RSI_PERIOD);
                let rsi = technical::rsi(&bars()?.close, period)?;
                let hit = if self.condition == AlertCondition::RsiBelow {
                    rsi <= target
                } else {
                    rsi >= target
                };
                Some(level(rsi, hit))
            }
            AlertCondition::CrossAboveSma => cross(&bars()?.close, self.length(), false, true),
            AlertCondition::CrossBelowSma => cross(&bars()?.close, self.length(), false, false),
            AlertCondition::CrossAboveEma => cross(&bars()?.close, self.length(), true, true),
            AlertCondition::CrossBelowEma => cross(&bars()?.close, self.length(), true, false),
            AlertCondition::BreakoutHigh => breakout(bars()?, self.length(), true),
            AlertCondition::BreakdownLow => breakout(bars()?, self.length(), false),
            AlertCondition::VolumeSpike => {
                let b = bars()?;
                let n = self.period().unwrap_or(VOLUME_PERIOD);
                if n == 0 || b.len() < n + 1 {
                    return None;
                }
                let last = b.len() - 1;
                let avg = b.volume[last - n..last].iter().sum::<u64>() as f64 / n as f64;
                if avg <= 0.0 {
                    return None;
                }
                let multiple = b.volume[last] as f64 / avg;
                Some(Reading {
                    hit: multiple >= target,
                    value: multiple,
                    reference: Some(avg),
                })
            }
            AlertCondition::ResultsIn => {
                let days = (data.next_results? - data.today?).num_days();
                (days >= 0).then(|| level(days as f64, days as f64 <= target))
            }
        }
    }

    /// The condition in words, e.g. "RSI(14) below 30".
    pub fn describe(&self, lang: Lang) -> String {
        let t = self.target;
        let n = self.length();
        let period = self.period().unwrap_or_default();
        match self.condition {
            AlertCondition::Above => tf_in(lang, Msg::AlertAbove, &[&format!("{t:.2}")]),
            AlertCondition::Below => tf_in(lang, Msg::AlertBelow, &[&format!("{t:.2}")]),
            AlertCondition::RsiBelow => tf_in(lang, Msg::AlertRsiBelow, &[&period, &number(t)]),
            AlertCondition::RsiAbove => tf_in(lang, Msg::AlertRsiAbove, &[&period, &number(t)]),
            AlertCondition::CrossAboveSma => tf_in(lang, Msg::AlertCrossAbove, &[&n, &"SMA"]),
            AlertCondition::CrossBelowSma => tf_in(lang, Msg::AlertCrossBelow, &[&n, &"SMA"]),
            AlertCondition::CrossAboveEma => tf_in(lang, Msg::AlertCrossAbove, &[&n, &"EMA"]),
            AlertCondition::CrossBelowEma => tf_in(lang, Msg::AlertCrossBelow, &[&n, &"EMA"]),
            AlertCondition::BreakoutHigh => tf_in(lang, Msg::AlertBreakoutHigh, &[&n]),
            AlertCondition::BreakdownLow => tf_in(lang, Msg::AlertBreakdownLow, &[&n]),
            AlertCondition::ChangeAbove => {
                tf_in(lang, Msg::AlertChangeAbove, &[&format!("{t:+.2}%")])
            }
            AlertCondition::ChangeBelow => {
                tf_in(lang, Msg::AlertChangeBelow, &[&format!("{t:+.2}%")])
            }
            AlertCondition::VolumeSpike => {
                tf_in(lang, Msg::AlertVolumeSpike, &[&number(t), &period])
            }
            AlertCondition::ResultsIn => tf_in(lang, Msg::AlertResultsIn, &[&n]),
        }
    }

    /// A few characters for narrow screens (the TUI), e.g. `RSI14≤30`.
    pub fn short_label(&self) -> String {
        let t = self.target;
        let n = self.length();
        let period = self.period().unwrap_or_default();
        match self.condition {
            AlertCondition::Above => format!("≥ {}", number(t)),
            AlertCondition::Below => format!("≤ {}", number(t)),
            AlertCondition::RsiBelow => format!("RSI{period}≤{}", number(t)),
            AlertCondition::RsiAbove => format!("RSI{period}≥{}", number(t)),
            AlertCondition::CrossAboveSma => format!("↑SMA{n}"),
            AlertCondition::CrossBelowSma => format!("↓SMA{n}"),
            AlertCondition::CrossAboveEma => format!("↑EMA{n}"),
            AlertCondition::CrossBelowEma => format!("↓EMA{n}"),
            AlertCondition::BreakoutHigh => format!("↑{n}d high"),
            AlertCondition::BreakdownLow => format!("↓{n}d low"),
            AlertCondition::ChangeAbove => format!("day≥{t:+.1}%"),
            AlertCondition::ChangeBelow => format!("day≤{t:+.1}%"),
            AlertCondition::VolumeSpike => format!("vol≥{}×", number(t)),
            AlertCondition::ResultsIn => format!("results≤{n}d"),
        }
    }

    /// What the reading shows besides the price, e.g. "RSI 28.4" or
    /// "50-day SMA 4012.30". `None` for price-level alerts.
    pub fn reading_text(&self, r: &Reading) -> Option<String> {
        let reference = r.reference.unwrap_or(f64::NAN);
        Some(match self.condition {
            AlertCondition::Above | AlertCondition::Below => return None,
            AlertCondition::RsiBelow | AlertCondition::RsiAbove => format!("RSI {:.1}", r.value),
            AlertCondition::CrossAboveSma | AlertCondition::CrossBelowSma => {
                format!("{}-day SMA {:.2}", self.length(), reference)
            }
            AlertCondition::CrossAboveEma | AlertCondition::CrossBelowEma => {
                format!("{}-day EMA {:.2}", self.length(), reference)
            }
            AlertCondition::BreakoutHigh => {
                format!("{}-day high {:.2}", self.length(), reference)
            }
            AlertCondition::BreakdownLow => format!("{}-day low {:.2}", self.length(), reference),
            AlertCondition::ChangeAbove | AlertCondition::ChangeBelow => {
                format!("day {:+.2}%", r.value)
            }
            AlertCondition::VolumeSpike => format!("volume {:.1}× average", r.value),
            AlertCondition::ResultsIn => match r.value as i64 {
                0 => "results today".to_string(),
                1 => "results tomorrow".to_string(),
                d => format!("results in {d} days"),
            },
        })
    }
}

fn valid(p: &f64) -> bool {
    p.is_finite() && *p > 0.0
}

/// `30` rather than `30.00` for whole numbers.
fn number(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// A close crossing its `n`-bar moving average on the last bar: on the
/// other side (or touching) one bar earlier, strictly beyond it now.
fn cross(closes: &[f64], n: usize, ema: bool, above: bool) -> Option<Reading> {
    let len = closes.len();
    if n < 2 || len < n + 1 {
        return None;
    }
    let ma = |c: &[f64]| {
        if ema {
            technical::ema(c, n)
        } else {
            technical::sma(c, n)
        }
    };
    let (now, before) = (ma(closes)?, ma(&closes[..len - 1])?);
    let (close, prev) = (closes[len - 1], closes[len - 2]);
    let hit = if above {
        prev <= before && close > now
    } else {
        prev >= before && close < now
    };
    Some(Reading {
        hit,
        value: close,
        reference: Some(now),
    })
}

/// The last close beyond the highest high (`up`) or lowest low of the
/// previous `n` bars.
fn breakout(bars: &Bars, n: usize, up: bool) -> Option<Reading> {
    let len = bars.len();
    if n == 0 || len < n + 1 {
        return None;
    }
    let close = bars.close[len - 1];
    let (hit, level) = if up {
        let high = bars.high[len - 1 - n..len - 1]
            .iter()
            .copied()
            .fold(f64::MIN, f64::max);
        (close > high, high)
    } else {
        let low = bars.low[len - 1 - n..len - 1]
            .iter()
            .copied()
            .fold(f64::MAX, f64::min);
        (close < low, low)
    };
    Some(Reading {
        hit,
        value: close,
        reference: Some(level),
    })
}

/// The chart range that holds at least `bars` daily bars.
pub fn chart_range(bars: usize) -> &'static str {
    match bars {
        0..=100 => "6mo",
        101..=230 => "1y",
        231..=480 => "2y",
        _ => "5y",
    }
}

/// Bring the last daily bar up to the live quote. Yahoo's chart is cached
/// for minutes and sometimes leaves out the live bar. `quote_day` is the
/// exchange date of the quote's last trade: a quote from the last bar's
/// session updates it, a quote from a later session becomes a new bar, and
/// an older quote changes nothing. Without a trade time, only a bar already
/// dated `session` (today) is updated — never a bar invented from a quote
/// that may be yesterday's close.
pub fn patch_live_bar(
    bars: &mut Bars,
    quote: &crate::api::Quote,
    session: NaiveDate,
    quote_day: Option<NaiveDate>,
) {
    let Some(price) = quote.regular_market_price.filter(valid) else {
        return;
    };
    let high = quote
        .regular_market_day_high
        .filter(valid)
        .unwrap_or(price)
        .max(price);
    let low = quote
        .regular_market_day_low
        .filter(valid)
        .unwrap_or(price)
        .min(price);
    let volume = quote.regular_market_volume.unwrap_or(0);
    let last_day = bars.time.last().map(|&t| bar_date(t));
    let day = quote_day.unwrap_or(session);
    if last_day == Some(day) {
        let i = bars.len() - 1;
        bars.close[i] = price;
        bars.high[i] = bars.high[i].max(high);
        bars.low[i] = bars.low[i].min(low);
        bars.volume[i] = bars.volume[i].max(volume);
    } else if quote_day.is_some() && last_day.is_none_or(|d| d < day) {
        let open_price = quote.regular_market_open.filter(valid).unwrap_or(price);
        let t = day
            .and_hms_opt(0, 0, 0)
            .map_or(0, |d| d.and_utc().timestamp());
        bars.time.push(t);
        bars.open.push(open_price);
        bars.high.push(high.max(open_price));
        bars.low.push(low.min(open_price));
        bars.close.push(price);
        bars.volume.push(volume);
    }
}

/// The session date of a daily bar. Yahoo stamps daily bars at the
/// session's open, which falls on the same UTC date for NSE, NYSE and
/// crypto.
fn bar_date(t: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

/// The exchange whose hours apply to a symbol, or None for instruments that
/// trade around the clock (crypto, FX) and other exchanges' listings.
pub fn alert_market(symbol: &str) -> Option<Market> {
    let s = symbol.to_uppercase();
    if s.ends_with("-USD")
        || s.ends_with("-INR")
        || s.contains('=')
        || matches!(
            crate::market::AssetClass::of(&s),
            crate::market::AssetClass::Crypto | crate::market::AssetClass::Fx
        )
    {
        None
    } else if s.ends_with(".NS")
        || s.ends_with(".BO")
        || crate::offmarket::is_fund(&s)
        || s.starts_with("^NSE")
        || s.starts_with("^CNX")
        || s == "^BSESN"
    {
        Some(Market::In)
    } else if let Some(x) = crate::market::exchange_of(&s) {
        Some(Market::Global(x))
    } else if s.contains('.') {
        None
    } else {
        Some(Market::Us)
    }
}

/// Check a new alert's numbers and fill in its period.
pub fn validate(
    condition: AlertCondition,
    target: f64,
    period: Option<usize>,
) -> Result<(f64, Option<usize>)> {
    if !target.is_finite() {
        bail!("The alert's number must be finite");
    }
    if period.is_some() && condition.default_period().is_none() {
        bail!(
            "--period only applies to rsi-below, rsi-above and volume-spike (for {} the number is the length itself)",
            condition
        );
    }
    let whole = |what: &str, lo: f64, hi: f64| -> Result<()> {
        if target.fract() != 0.0 || target < lo || target > hi {
            bail!("{} takes {} from {} to {}", condition, what, lo, hi);
        }
        Ok(())
    };
    let target = match condition {
        AlertCondition::Above | AlertCondition::Below => {
            if target <= 0.0 {
                bail!("A price level must be above 0");
            }
            target
        }
        AlertCondition::RsiBelow | AlertCondition::RsiAbove => {
            if target <= 0.0 || target >= 100.0 {
                bail!("RSI runs from 0 to 100 — pick a level between them, e.g. 30 or 70");
            }
            target
        }
        AlertCondition::CrossAboveSma
        | AlertCondition::CrossBelowSma
        | AlertCondition::CrossAboveEma
        | AlertCondition::CrossBelowEma => {
            whole("a moving-average length in days", 2.0, 400.0)?;
            target
        }
        AlertCondition::BreakoutHigh | AlertCondition::BreakdownLow => {
            whole("a look-back in days", 2.0, 1000.0)?;
            target
        }
        // The direction comes from the condition: `change-below 3` and
        // `change-below -3` both mean a fall of 3% or more.
        AlertCondition::ChangeAbove | AlertCondition::ChangeBelow => {
            if target == 0.0 || target.abs() >= 100.0 {
                bail!("A day change must be a percentage between 0 and 100, e.g. 3");
            }
            if condition == AlertCondition::ChangeAbove {
                target.abs()
            } else {
                -target.abs()
            }
        }
        AlertCondition::VolumeSpike => {
            if !(1.0..=100.0).contains(&target) {
                bail!("volume-spike takes a multiple of the average volume from 1 to 100, e.g. 2");
            }
            target
        }
        AlertCondition::ResultsIn => {
            whole("a number of days", 0.0, 90.0)?;
            target
        }
    };
    let period = match condition.default_period() {
        Some(default) => {
            let p = period.unwrap_or(default);
            if !(2..=200).contains(&p) {
                bail!("--period must be from 2 to 200");
            }
            Some(p)
        }
        None => None,
    };
    Ok((target, period))
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AlertStore {
    pub alerts: Vec<Alert>,
}

impl AlertStore {
    pub fn load() -> Result<Self> {
        let mut store: Self = crate::store::load("alerts.json")?;
        store.repair_symbols();
        Ok(store)
    }

    /// Undo the market suffix older versions put on namespaced symbols
    /// (`alert add MF:120503 ...` stored `MF:120503.NS`). The repair is saved
    /// with the next change to the alerts.
    fn repair_symbols(&mut self) -> bool {
        let mut changed = false;
        for a in &mut self.alerts {
            if let Some(fixed) = crate::market::repair_namespaced(&a.symbol) {
                a.symbol = fixed;
                changed = true;
            }
        }
        changed
    }

    pub fn save(&self) -> Result<()> {
        crate::store::save("alerts.json", self, false)
    }

    pub fn add(&mut self, symbol: &str, condition: AlertCondition, target: f64) {
        self.add_with_period(symbol, condition, target, condition.default_period());
    }

    pub fn add_with_period(
        &mut self,
        symbol: &str,
        condition: AlertCondition,
        target: f64,
        period: Option<usize>,
    ) {
        self.alerts.push(Alert {
            symbol: symbol.to_uppercase(),
            condition,
            target,
            period,
            created_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        });
    }

    pub fn remove_by_index(&mut self, index: usize) -> bool {
        if index < self.alerts.len() {
            self.alerts.remove(index);
            true
        } else {
            false
        }
    }

    /// Price-level alerts on `symbol` that `price` triggers.
    pub fn check(&self, symbol: &str, price: f64) -> Vec<&Alert> {
        let data = MarketData {
            price: Some(price),
            ..Default::default()
        };
        self.alerts
            .iter()
            .filter(|a| {
                a.symbol == symbol.to_uppercase()
                    && a.condition.is_price()
                    && a.evaluate(&data).is_some_and(|r| r.hit)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(condition: AlertCondition, target: f64) -> Alert {
        let (target, period) = validate(condition, target, None).unwrap();
        Alert {
            symbol: "TEST.NS".into(),
            condition,
            target,
            period,
            created_at: "2026-10-21 10:00".into(),
        }
    }

    fn closes(closes: &[f64]) -> MarketData {
        MarketData {
            bars: Some(Bars::from_closes(closes)),
            ..Default::default()
        }
    }

    fn hit(a: &Alert, data: &MarketData) -> bool {
        a.evaluate(data).expect("enough data").hit
    }

    #[test]
    fn test_add_alert() {
        let mut store = AlertStore::default();
        store.add("RELIANCE.NS", AlertCondition::Above, 1500.0);
        assert_eq!(store.alerts.len(), 1);
        assert_eq!(store.alerts[0].symbol, "RELIANCE.NS");
        assert_eq!(store.alerts[0].target, 1500.0);
        assert_eq!(store.alerts[0].period, None);
    }

    #[test]
    fn fund_codes_stored_with_a_market_suffix_are_repaired() {
        let mut store = AlertStore::default();
        store.add("MF:120503.NS", AlertCondition::Above, 60.0);
        store.add("MF:INF179K01UT0-USD", AlertCondition::Below, 40.0);
        store.add("TCS.NS", AlertCondition::Above, 4000.0);
        assert!(store.repair_symbols());
        let symbols: Vec<&str> = store.alerts.iter().map(|a| a.symbol.as_str()).collect();
        assert_eq!(symbols, ["MF:120503", "MF:INF179K01UT0", "TCS.NS"]);
        assert!(!store.repair_symbols(), "idempotent");
    }

    #[test]
    fn test_check_above_triggered() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Above, 100.0);
        let triggered = store.check("TEST.NS", 105.0);
        assert_eq!(triggered.len(), 1);
    }

    #[test]
    fn test_check_above_not_triggered() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Above, 100.0);
        let triggered = store.check("TEST.NS", 95.0);
        assert_eq!(triggered.len(), 0);
    }

    #[test]
    fn test_check_below_triggered() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Below, 100.0);
        let triggered = store.check("TEST.NS", 95.0);
        assert_eq!(triggered.len(), 1);
    }

    #[test]
    fn test_check_wrong_symbol() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Above, 100.0);
        let triggered = store.check("OTHER.NS", 200.0);
        assert_eq!(triggered.len(), 0);
    }

    #[test]
    fn test_remove_by_index() {
        let mut store = AlertStore::default();
        store.add("A.NS", AlertCondition::Above, 100.0);
        store.add("B.NS", AlertCondition::Below, 50.0);
        assert!(store.remove_by_index(0));
        assert_eq!(store.alerts.len(), 1);
        assert_eq!(store.alerts[0].symbol, "B.NS");
    }

    #[test]
    fn test_remove_invalid_index() {
        let mut store = AlertStore::default();
        store.add("A.NS", AlertCondition::Above, 100.0);
        assert!(!store.remove_by_index(5));
        assert_eq!(store.alerts.len(), 1);
    }

    #[test]
    fn test_multiple_alerts_same_symbol() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Above, 110.0);
        store.add("TEST.NS", AlertCondition::Below, 90.0);
        // Price at 85 triggers below only
        let triggered = store.check("TEST.NS", 85.0);
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].condition, AlertCondition::Below);
        // Price at 115 triggers above only
        let triggered = store.check("TEST.NS", 115.0);
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].condition, AlertCondition::Above);
    }

    #[test]
    fn test_condition_display() {
        assert_eq!(format!("{}", AlertCondition::Above), "above");
        assert_eq!(format!("{}", AlertCondition::Below), "below");
        assert_eq!(
            format!("{}", AlertCondition::CrossAboveSma),
            "cross-above-sma"
        );
    }

    #[test]
    fn every_condition_parses_from_its_name() {
        for c in AlertCondition::ALL {
            assert_eq!(AlertCondition::parse(c.name()), Some(c));
        }
        assert_eq!(AlertCondition::parse(">"), Some(AlertCondition::Above));
        assert_eq!(
            AlertCondition::parse("RSI-Below"),
            Some(AlertCondition::RsiBelow)
        );
        assert_eq!(AlertCondition::parse("sideways"), None);
    }

    #[test]
    fn old_alerts_json_loads_unchanged() {
        // alerts.json as 0.4 wrote it: no `period`, Above/Below only.
        let old = r#"{"alerts":[
            {"symbol":"TCS.NS","condition":"Above","target":4200.0,"created_at":"2026-01-02 09:30"},
            {"symbol":"AAPL","condition":"Below","target":180.5,"created_at":"2026-01-03 21:00"}
        ]}"#;
        let store: AlertStore = serde_json::from_str(old).unwrap();
        assert_eq!(store.alerts.len(), 2);
        assert_eq!(store.alerts[0].condition, AlertCondition::Above);
        assert_eq!(store.alerts[1].target, 180.5);
        assert_eq!(store.alerts[1].period, None);
        // Saved again, an old alert is written exactly as before.
        let resaved = serde_json::to_value(&store.alerts[0]).unwrap();
        assert_eq!(
            resaved,
            serde_json::json!({"symbol":"TCS.NS","condition":"Above","target":4200.0,"created_at":"2026-01-02 09:30"})
        );
        // New alerts round-trip with their period.
        let mut store = store;
        store.add("INFY.NS", AlertCondition::RsiBelow, 30.0);
        let json = serde_json::to_string(&store).unwrap();
        let back: AlertStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.alerts[2].condition, AlertCondition::RsiBelow);
        assert_eq!(back.alerts[2].period, Some(14));
    }

    #[test]
    fn rsi_alerts_compare_the_level() {
        // Steady falls: RSI 0. Steady rises: RSI 100.
        let falling: Vec<f64> = (0..40).map(|i| 200.0 - i as f64).collect();
        let rising: Vec<f64> = (0..40).map(|i| 100.0 + i as f64).collect();
        let below = alert(AlertCondition::RsiBelow, 30.0);
        let above = alert(AlertCondition::RsiAbove, 70.0);
        assert!(hit(&below, &closes(&falling)));
        assert!(!hit(&below, &closes(&rising)));
        assert!(hit(&above, &closes(&rising)));
        assert!(!hit(&above, &closes(&falling)));
        let r = below.evaluate(&closes(&falling)).unwrap();
        assert_eq!(r.value, 0.0);
        // Too little history for RSI(14): no reading rather than a guess.
        assert!(below.evaluate(&closes(&falling[..10])).is_none());
        // A custom period is used.
        let mut short = below.clone();
        short.period = Some(3);
        assert!(short.evaluate(&closes(&falling[..10])).is_some());
    }

    /// 30 bars at 100, then a jump to 110 and more bars at 110.
    fn step_up() -> Vec<f64> {
        let mut c = vec![100.0; 30];
        c.extend([110.0; 5]);
        c
    }

    #[test]
    fn sma_cross_fires_only_on_the_crossing_bar() {
        let a = alert(AlertCondition::CrossAboveSma, 20.0);
        let c = step_up();
        // Bar 30 is the first close above the 20-day SMA.
        assert!(!hit(&a, &closes(&c[..30])));
        assert!(hit(&a, &closes(&c[..31])));
        // Still above on the following bars, but no new cross.
        for end in 32..=c.len() {
            assert!(!hit(&a, &closes(&c[..end])), "bar {end}");
        }
        let r = a.evaluate(&closes(&c[..31])).unwrap();
        assert_eq!(r.value, 110.0);
        assert_eq!(r.reference, Some(100.5));
        // The opposite direction never fires on a rise.
        let down = alert(AlertCondition::CrossBelowSma, 20.0);
        for end in 21..=c.len() {
            assert!(!hit(&down, &closes(&c[..end])));
        }
        // Not enough bars for a 20-day average one bar back.
        assert!(a.evaluate(&closes(&c[..20])).is_none());
    }

    #[test]
    fn ema_cross_below_fires_on_the_crossing_bar() {
        let mut c = vec![100.0; 30];
        c.extend([90.0, 89.0, 88.0]);
        let a = alert(AlertCondition::CrossBelowEma, 10.0);
        assert!(!hit(&a, &closes(&c[..30])));
        assert!(hit(&a, &closes(&c[..31])));
        assert!(!hit(&a, &closes(&c[..32])));
        assert!(!hit(&a, &closes(&c[..33])));
        let up = alert(AlertCondition::CrossAboveEma, 10.0);
        assert!(!hit(&up, &closes(&c[..31])));
    }

    #[test]
    fn a_touch_then_a_close_beyond_counts_as_a_cross() {
        // Close equal to the average one bar back, above it now.
        let mut c = vec![100.0; 21];
        c.push(101.0);
        assert!(hit(&alert(AlertCondition::CrossAboveSma, 5.0), &closes(&c)));
    }

    #[test]
    fn breakouts_compare_with_the_previous_bars_only() {
        let mut bars = Bars::from_closes(&[100.0, 104.0, 102.0, 103.0, 101.0]);
        // Highs/lows a little around the closes.
        bars.high = vec![101.0, 105.0, 103.0, 104.0, 102.0];
        bars.low = vec![99.0, 103.0, 101.0, 102.0, 100.0];
        let data = |b: &Bars| MarketData {
            bars: Some(b.clone()),
            ..Default::default()
        };
        let high = alert(AlertCondition::BreakoutHigh, 4.0);
        let low = alert(AlertCondition::BreakdownLow, 4.0);
        assert!(!hit(&high, &data(&bars)));
        assert!(!hit(&low, &data(&bars)));
        // Today closes above the 4-day high of 105 (its own high doesn't count).
        let mut up = bars.clone();
        up.close.push(106.0);
        up.high.push(107.0);
        up.low.push(102.0);
        up.open.push(102.0);
        up.volume.push(1000);
        up.time.push(0);
        let r = high.evaluate(&data(&up)).unwrap();
        assert!(r.hit);
        assert_eq!(r.reference, Some(105.0));
        // A close below the previous 4 lows (min 100) breaks down.
        let mut down = bars.clone();
        down.close.push(99.5);
        down.high.push(101.0);
        down.low.push(99.0);
        down.open.push(101.0);
        down.volume.push(1000);
        down.time.push(0);
        assert!(hit(&low, &data(&down)));
        assert!(!hit(&high, &data(&down)));
        // Not enough history.
        assert!(high.evaluate(&data(&bars.slice(0..4))).is_none());
    }

    #[test]
    fn day_change_alerts_use_the_sign_of_their_direction() {
        let up = alert(AlertCondition::ChangeAbove, 3.0);
        let down = alert(AlertCondition::ChangeBelow, 3.0);
        assert_eq!(down.target, -3.0);
        assert_eq!(alert(AlertCondition::ChangeBelow, -3.0).target, -3.0);
        assert_eq!(alert(AlertCondition::ChangeAbove, -2.0).target, 2.0);
        let day = |c: f64| MarketData {
            change_percent: Some(c),
            ..Default::default()
        };
        assert!(hit(&up, &day(3.0)));
        assert!(!hit(&up, &day(2.9)));
        assert!(!hit(&up, &day(-5.0)));
        assert!(hit(&down, &day(-3.5)));
        assert!(!hit(&down, &day(-1.0)));
        assert!(!hit(&down, &day(4.0)));
        assert!(up.evaluate(&MarketData::default()).is_none());
    }

    #[test]
    fn volume_spikes_compare_with_the_previous_average() {
        let mut bars = Bars::from_closes(&[100.0; 21]);
        bars.volume = vec![1000; 21];
        let a = alert(AlertCondition::VolumeSpike, 2.0);
        assert_eq!(a.period, Some(20));
        let data = |b: &Bars| MarketData {
            bars: Some(b.clone()),
            ..Default::default()
        };
        assert!(!hit(&a, &data(&bars)));
        bars.volume[20] = 2500;
        let r = a.evaluate(&data(&bars)).unwrap();
        assert!(r.hit);
        assert_eq!(r.value, 2.5);
        assert_eq!(r.reference, Some(1000.0));
        // No volume history (an index): no reading.
        bars.volume = vec![0; 21];
        assert!(a.evaluate(&data(&bars)).is_none());
    }

    #[test]
    fn results_alerts_count_days_to_the_next_results() {
        let a = alert(AlertCondition::ResultsIn, 7.0);
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let data = |next: &str| MarketData {
            next_results: Some(d(next)),
            today: Some(d("2026-10-21")),
            ..Default::default()
        };
        assert!(hit(&a, &data("2026-10-28")));
        assert!(hit(&a, &data("2026-10-21")));
        assert!(!hit(&a, &data("2026-10-29")));
        assert_eq!(a.evaluate(&data("2026-10-25")).unwrap().value, 4.0);
        // A past date or no date: nothing to compare.
        assert!(a.evaluate(&data("2026-10-20")).is_none());
        assert!(a.evaluate(&MarketData::default()).is_none());
    }

    #[test]
    fn validation_rejects_numbers_that_make_no_sense() {
        assert!(validate(AlertCondition::Above, 0.0, None).is_err());
        assert!(validate(AlertCondition::RsiBelow, 120.0, None).is_err());
        assert!(validate(AlertCondition::CrossAboveSma, 20.5, None).is_err());
        assert!(validate(AlertCondition::CrossAboveSma, 1.0, None).is_err());
        assert!(validate(AlertCondition::BreakoutHigh, 52.0, None).is_ok());
        assert!(validate(AlertCondition::ChangeAbove, 0.0, None).is_err());
        assert!(validate(AlertCondition::VolumeSpike, 0.5, None).is_err());
        assert!(validate(AlertCondition::ResultsIn, 91.0, None).is_err());
        // --period only where it means something.
        assert!(validate(AlertCondition::CrossAboveSma, 50.0, Some(10)).is_err());
        assert_eq!(
            validate(AlertCondition::RsiAbove, 70.0, Some(9)).unwrap(),
            (70.0, Some(9))
        );
        assert!(validate(AlertCondition::RsiAbove, 70.0, Some(1)).is_err());
    }

    #[test]
    fn conditions_read_as_words() {
        let en = Lang::En;
        assert_eq!(
            alert(AlertCondition::Above, 4200.0).describe(en),
            "above 4200.00"
        );
        assert_eq!(
            alert(AlertCondition::RsiBelow, 30.0).describe(en),
            "RSI(14) below 30"
        );
        assert_eq!(
            alert(AlertCondition::CrossAboveSma, 50.0).describe(en),
            "close crosses above the 50-day SMA"
        );
        assert_eq!(
            alert(AlertCondition::CrossBelowEma, 20.0).describe(en),
            "close crosses below the 20-day EMA"
        );
        assert_eq!(
            alert(AlertCondition::BreakoutHigh, 52.0).describe(en),
            "close above the previous 52-day high"
        );
        assert_eq!(
            alert(AlertCondition::ChangeBelow, 3.0).describe(en),
            "day change of -3.00% or worse"
        );
        assert_eq!(
            alert(AlertCondition::VolumeSpike, 2.5).describe(en),
            "volume 2.5× its 20-day average"
        );
        assert_eq!(
            alert(AlertCondition::ResultsIn, 7.0).describe(en),
            "results due within 7 days"
        );
        // Every condition has words and a short label in both languages.
        for c in AlertCondition::ALL {
            let a = alert(
                c,
                if c == AlertCondition::ResultsIn {
                    3.0
                } else {
                    50.0
                },
            );
            assert!(!a.describe(Lang::Hi).is_empty());
            assert!(!a.short_label().is_empty());
        }
    }

    #[test]
    fn the_live_quote_updates_or_adds_todays_bar() {
        use crate::api::Quote;
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        // Two bars: 2024-01-01 and 2024-01-02.
        let bars = Bars::from_closes(&[100.0, 101.0]);
        let quote = Quote {
            regular_market_price: Some(105.0),
            regular_market_day_high: Some(106.0),
            regular_market_day_low: Some(100.5),
            regular_market_volume: Some(5000),
            ..Default::default()
        };
        // A quote from the last bar's session updates it.
        let mut same = bars.clone();
        patch_live_bar(&mut same, &quote, d("2024-01-02"), Some(d("2024-01-02")));
        assert_eq!(same.len(), 2);
        assert_eq!(same.close[1], 105.0);
        assert_eq!(same.high[1], 106.0);
        assert_eq!(same.volume[1], 5000);
        // A quote from a newer session becomes that session's bar.
        let mut next = bars.clone();
        patch_live_bar(&mut next, &quote, d("2024-01-03"), Some(d("2024-01-03")));
        assert_eq!(next.len(), 3);
        assert_eq!(next.close[2], 105.0);
        assert_eq!(bar_date(next.time[2]), d("2024-01-03"));
        // Before the open (or on a weekend or an unlisted holiday) the quote
        // is still the last session's: no phantom bar for today.
        let mut early = bars.clone();
        patch_live_bar(&mut early, &quote, d("2024-01-03"), Some(d("2024-01-02")));
        assert_eq!(early.len(), 2);
        assert_eq!(early.close[1], 105.0);
        // A quote older than the chart changes nothing.
        let mut old = bars.clone();
        patch_live_bar(&mut old, &quote, d("2024-01-02"), Some(d("2024-01-01")));
        assert_eq!(old.close, vec![100.0, 101.0]);
        // No trade time: today's bar is updated, but none is invented.
        let mut untimed = bars.clone();
        patch_live_bar(&mut untimed, &quote, d("2024-01-03"), None);
        assert_eq!(untimed.close, vec![100.0, 101.0]);
        patch_live_bar(&mut untimed, &quote, d("2024-01-02"), None);
        assert_eq!(untimed.close, vec![100.0, 105.0]);
        // No live price: nothing to patch.
        let mut none = bars.clone();
        patch_live_bar(
            &mut none,
            &Quote::default(),
            d("2024-01-02"),
            Some(d("2024-01-02")),
        );
        assert_eq!(none.close, vec![100.0, 101.0]);
    }

    #[test]
    fn a_weekend_quote_does_not_fake_a_cross() {
        // The review's case: Friday's close sits above a falling 5-day SMA
        // only if Friday's bar is counted twice.
        use crate::api::Quote;
        let mut bars = Bars::from_closes(&[100.0, 110.0, 100.0, 100.0, 100.0, 101.0]);
        let friday = bar_date(*bars.time.last().unwrap());
        let a = alert(AlertCondition::CrossAboveSma, 5.0);
        let data = |b: &Bars| MarketData {
            bars: Some(b.clone()),
            ..Default::default()
        };
        assert!(!hit(&a, &data(&bars)));
        let quote = Quote {
            regular_market_price: Some(101.0),
            ..Default::default()
        };
        let saturday = friday.succ_opt().unwrap();
        patch_live_bar(&mut bars, &quote, saturday, Some(friday));
        assert_eq!(bars.len(), 6);
        assert!(!hit(&a, &data(&bars)));
    }

    #[test]
    fn chart_ranges_hold_the_bars_needed() {
        assert_eq!(
            chart_range(alert(AlertCondition::RsiBelow, 30.0).bars_needed()),
            "6mo"
        );
        assert_eq!(
            chart_range(alert(AlertCondition::CrossAboveSma, 200.0).bars_needed()),
            "1y"
        );
        assert_eq!(
            chart_range(alert(AlertCondition::CrossAboveEma, 100.0).bars_needed()),
            "2y"
        );
        assert_eq!(
            chart_range(alert(AlertCondition::BreakoutHigh, 600.0).bars_needed()),
            "5y"
        );
        assert_eq!(alert(AlertCondition::Above, 1.0).bars_needed(), 0);
    }

    #[test]
    fn alert_markets_follow_the_symbol() {
        assert_eq!(alert_market("TCS.NS"), Some(Market::In));
        assert_eq!(alert_market("RELIANCE.BO"), Some(Market::In));
        assert_eq!(alert_market("MF:120503"), Some(Market::In));
        assert_eq!(alert_market("^NSEI"), Some(Market::In));
        assert_eq!(alert_market("AAPL"), Some(Market::Us));
        assert_eq!(alert_market("^GSPC"), Some(Market::Us));
        assert_eq!(alert_market("BTC-USD"), None);
        assert_eq!(alert_market("BTC-EUR"), None);
        assert_eq!(alert_market("ETH-USDT"), None);
        assert_eq!(alert_market("USDINR=X"), None);
        assert_eq!(alert_market("BRK-B"), Some(Market::Us));
        assert_eq!(alert_market("BAJAJ-AUTO.NS"), Some(Market::In));
        assert_eq!(
            alert_market("VOD.L"),
            Some(Market::Global(crate::market::Exchange::Uk))
        );
        assert_eq!(
            alert_market("^N225"),
            Some(Market::Global(crate::market::Exchange::Jp))
        );
        // COMEX futures trade nearly around the clock: always checked.
        assert_eq!(alert_market("GC=F"), None);
    }
}
