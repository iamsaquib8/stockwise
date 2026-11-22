//! Automatic monitoring of every holding, with no setup: large day drops,
//! new 52-week lows, falls from cost or from the peak, gaps at the open,
//! dates tomorrow (results, ex-dividend, record date, split), promoter
//! pledge and shareholding changes, analyst rating changes and insider
//! trades.
//!
//! Everything here is pure: the worker and `stockwise monitor` gather the
//! data, call the detectors and remember each [`Signal::key`] once it has
//! been delivered, so a signal is sent once. Thresholds come from
//! `[monitor]` in the config file or `STOCKWISE_MONITOR_<KEY>`.
//!
//! Signals state facts (what moved, by how much, against what); what to do
//! about them is the holder's call.

use crate::notify::{Message, Priority};
use crate::research::Series;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Monitor thresholds. Percentages are positive numbers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Thresholds {
    /// Monitoring on at all.
    pub enabled: bool,
    /// A day fall of at least this many percent...
    pub day_drop_pct: f64,
    /// ...or of at least this many standard deviations of the stock's own
    /// daily moves (and at least 1.5%).
    pub day_drop_sigma: f64,
    /// Price this far below the average cost (then every further 10%).
    pub from_cost_pct: f64,
    /// Price this far below the 52-week closing high (then every further 10%).
    pub from_peak_pct: f64,
    /// Open this far from the previous close, either way.
    pub gap_pct: f64,
    /// Send the after-close digest.
    pub digest: bool,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            enabled: true,
            day_drop_pct: 5.0,
            day_drop_sigma: 2.0,
            from_cost_pct: 20.0,
            from_peak_pct: 25.0,
            gap_pct: 3.0,
            digest: true,
        }
    }
}

impl Thresholds {
    /// From the environment and `[monitor]` in the config file.
    pub fn from_config() -> Self {
        Self::from_lookup(crate::config::monitor_setting)
    }

    /// From a key lookup (`day_drop_pct`, ...). Values that don't parse, or
    /// percentages outside (0, 100), keep the default.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let pct = |key: &str, default: f64| {
            lookup(key)
                .and_then(|v| v.trim().trim_end_matches('%').parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0 && *v < 100.0)
                .unwrap_or(default)
        };
        let flag = |key: &str, default: bool| {
            lookup(key)
                .map(|v| {
                    !matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "0" | "false" | "no" | "off"
                    )
                })
                .unwrap_or(default)
        };
        Self {
            enabled: flag("enabled", d.enabled),
            day_drop_pct: pct("day_drop_pct", d.day_drop_pct),
            day_drop_sigma: pct("day_drop_sigma", d.day_drop_sigma),
            from_cost_pct: pct("from_cost_pct", d.from_cost_pct),
            from_peak_pct: pct("from_peak_pct", d.from_peak_pct),
            gap_pct: pct("gap_pct", d.gap_pct),
            digest: flag("digest", d.digest),
        }
    }
}

/// What a signal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    DayDrop,
    NewLow,
    BelowCost,
    FromPeak,
    GapDown,
    GapUp,
    EventTomorrow,
    Pledge,
    Shareholding,
    Rating,
    Insider,
}

impl SignalKind {
    pub fn label(self) -> &'static str {
        match self {
            SignalKind::DayDrop => "day drop",
            SignalKind::NewLow => "52-week low",
            SignalKind::BelowCost => "below cost",
            SignalKind::FromPeak => "off the high",
            SignalKind::GapDown => "gap down",
            SignalKind::GapUp => "gap up",
            SignalKind::EventTomorrow => "tomorrow",
            SignalKind::Pledge => "pledge",
            SignalKind::Shareholding => "shareholding",
            SignalKind::Rating => "rating",
            SignalKind::Insider => "insider trade",
        }
    }

    /// Price signals can be checked during the session; the rest after the
    /// close.
    pub fn intraday(self) -> bool {
        matches!(
            self,
            SignalKind::DayDrop | SignalKind::NewLow | SignalKind::GapDown | SignalKind::GapUp
        )
    }
}

/// One thing worth knowing about a holding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub symbol: String,
    pub kind: SignalKind,
    pub priority: Priority,
    /// One line, e.g. "TCS.NS fell 6.2% today".
    pub title: String,
    /// The numbers behind it.
    pub detail: String,
    /// Remembered once delivered; the same key is never sent twice.
    pub key: String,
}

impl Signal {
    pub fn message(&self) -> Message {
        Message::new(self.title.clone(), self.detail.clone()).with_priority(self.priority)
    }
}

/// Sample standard deviation of daily returns over the sessions before the
/// last one (up to a year), and that last return.
pub fn last_return_and_sigma(closes: &[f64]) -> Option<(f64, f64)> {
    let n = closes.len();
    if n < 22 {
        return None;
    }
    let rets: Vec<f64> = closes
        .windows(2)
        .filter(|w| w[0] > 0.0 && w[1].is_finite())
        .map(|w| w[1] / w[0] - 1.0)
        .collect();
    let (last, prior) = rets.split_last()?;
    let prior = &prior[prior.len().saturating_sub(250)..];
    if prior.len() < 20 {
        return None;
    }
    let mean = prior.iter().sum::<f64>() / prior.len() as f64;
    let var = prior.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (prior.len() - 1) as f64;
    Some((*last, var.sqrt()))
}

/// The deepest 10%-step at or past `threshold` that `fall_pct` reaches
/// (20 → 20, 34 → 30 with a 20 threshold), so each further 10% notifies once.
fn bucket(fall_pct: f64, threshold: f64) -> Option<u32> {
    if !fall_pct.is_finite() || fall_pct + 1e-9 < threshold {
        return None;
    }
    let steps = ((fall_pct + 1e-9 - threshold) / 10.0).floor().max(0.0);
    Some((threshold + steps * 10.0).round() as u32)
}

/// Price signals from daily bars whose last bar is the session `day` (the
/// latest close or the live price), with the holding's average cost in the
/// same currency as the bars.
pub fn price_signals(
    symbol: &str,
    s: &Series,
    avg_cost: Option<f64>,
    t: &Thresholds,
) -> Vec<Signal> {
    let mut out = Vec::new();
    let n = s.len();
    if n < 2 {
        return out;
    }
    let day = s.dates[n - 1];
    let price = s.close[n - 1];
    let prev = s.close[n - 2];
    if !(price > 0.0 && prev > 0.0) {
        return out;
    }
    let ret = price / prev - 1.0;
    let sigma = last_return_and_sigma(&s.close).map(|(_, sd)| sd);

    // Day drop: a fixed percentage, or an unusual fall for this stock.
    let big = ret * 100.0 <= -t.day_drop_pct;
    let unusual =
        sigma.is_some_and(|sd| sd > 0.0 && ret <= -t.day_drop_sigma * sd && ret <= -0.015);
    if big || unusual {
        let z = sigma
            .filter(|sd| *sd > 0.0)
            .map(|sd| format!(", {:.1}σ of its usual daily move", ret.abs() / sd))
            .unwrap_or_default();
        out.push(Signal {
            symbol: symbol.into(),
            kind: SignalKind::DayDrop,
            priority: if big {
                Priority::High
            } else {
                Priority::Normal
            },
            title: format!(
                "{symbol} fell {:.1}% on {}",
                ret.abs() * 100.0,
                day.format("%d %b")
            ),
            detail: format!("{prev:.2} → {price:.2}{z}."),
            key: format!("{symbol}:drop:{day}"),
        });
    }

    // New 52-week low: require bars spanning roughly a year before labeling "52-week".
    if n > 60 {
        let from = n.saturating_sub(253);
        let span_days = (day - s.dates[from]).num_days();
        if span_days >= 330 {
            let prior_low = s.low[from..n - 1]
                .iter()
                .chain(&s.close[from..n - 1])
                .copied()
                .filter(|v| *v > 0.0)
                .fold(f64::INFINITY, f64::min);
            if prior_low.is_finite() && price < prior_low {
                out.push(Signal {
                    symbol: symbol.into(),
                    kind: SignalKind::NewLow,
                    priority: Priority::Normal,
                    title: format!("{symbol} at a new 52-week low"),
                    detail: format!("{price:.2}, below the year's low of {prior_low:.2}."),
                    key: format!("{symbol}:low52:{day}"),
                });
            }
        }
    }

    // Below cost, in 10% steps past the threshold.
    if let Some(cost) = avg_cost.filter(|c| *c > 0.0) {
        let fall = (1.0 - price / cost) * 100.0;
        if let Some(b) = bucket(fall, t.from_cost_pct) {
            out.push(Signal {
                symbol: symbol.into(),
                kind: SignalKind::BelowCost,
                priority: Priority::Normal,
                title: format!("{symbol} is {fall:.0}% below your cost"),
                detail: format!("{price:.2} against an average cost of {cost:.2}."),
                key: format!("{symbol}:cost:{b}"),
            });
        }
    }

    // Off the 52-week closing high.
    let from = n.saturating_sub(252);
    let peak = s.close[from..].iter().copied().fold(f64::MIN, f64::max);
    if peak > 0.0 {
        let fall = (1.0 - price / peak) * 100.0;
        if let Some(b) = bucket(fall, t.from_peak_pct) {
            // A new step re-notifies; recovering above the threshold and
            // falling again inside a year is the same step (same peak).
            let peak_day =
                s.dates[from + s.close[from..].iter().position(|c| *c == peak).unwrap_or(0)];
            out.push(Signal {
                symbol: symbol.into(),
                kind: SignalKind::FromPeak,
                priority: Priority::Normal,
                title: format!("{symbol} is {fall:.0}% off its 52-week high"),
                detail: format!(
                    "{price:.2} against a closing high of {peak:.2} ({}).",
                    peak_day.format("%d %b %Y")
                ),
                key: format!("{symbol}:peak:{peak_day}:{b}"),
            });
        }
    }

    // Gap at the open.
    let open = s.open[n - 1];
    if open > 0.0 {
        let gap = (open / prev - 1.0) * 100.0;
        if gap.abs() >= t.gap_pct {
            let down = gap < 0.0;
            out.push(Signal {
                symbol: symbol.into(),
                kind: if down {
                    SignalKind::GapDown
                } else {
                    SignalKind::GapUp
                },
                priority: if down {
                    Priority::Normal
                } else {
                    Priority::Low
                },
                title: format!(
                    "{symbol} opened {:.1}% {}",
                    gap.abs(),
                    if down { "lower" } else { "higher" }
                ),
                detail: format!("Open {open:.2} after a close of {prev:.2}."),
                key: format!("{symbol}:gap:{day}"),
            });
        }
    }
    out
}

/// Results, ex-dividend, record date, split or bonus on `tomorrow` (the
/// next session date), from the calendar.
pub fn event_signals(events: &[crate::calendar::Event], tomorrow: NaiveDate) -> Vec<Signal> {
    use crate::calendar::EventKind;
    events
        .iter()
        .filter(|e| e.date == tomorrow)
        .filter_map(|e| {
            let record = e.detail.to_ascii_lowercase().contains("record");
            let what = match e.kind {
                EventKind::Results => "results",
                EventKind::ExDividend => "ex-dividend date",
                EventKind::Split => "split (ex-date)",
                EventKind::Bonus => "bonus (ex-date)",
                EventKind::Rights => "rights (ex-date)",
                _ if record => "record date",
                EventKind::BoardMeeting => "board meeting",
                _ => return None,
            };
            let detail = if e.detail.is_empty() {
                format!("{} ({}).", e.date.format("%a %d %b"), e.source)
            } else {
                format!(
                    "{} — {} ({}).",
                    e.date.format("%a %d %b"),
                    e.detail,
                    e.source
                )
            };
            Some(Signal {
                symbol: e.symbol.clone(),
                kind: SignalKind::EventTomorrow,
                priority: if e.kind == EventKind::Results {
                    Priority::Normal
                } else {
                    Priority::Low
                },
                title: format!("{}: {} on {}", e.symbol, what, e.date.format("%d %b")),
                detail,
                key: format!("{}:event:{:?}:{}", e.symbol, e.kind, e.date),
            })
        })
        .collect()
}

/// Promoter holding and pledge changes between the last two shareholding
/// quarters (oldest first). A change of 1 point or more in the promoter
/// stake, a pledge appearing, or institutional (FII + DII) holding moving 2
/// points or more.
pub fn ownership_signals(symbol: &str, holdings: &[crate::research::Holding]) -> Vec<Signal> {
    let mut out = Vec::new();
    let [.., prev, last] = holdings else {
        return out;
    };
    let q = last.date;
    if last.pledged == Some(true) && prev.pledged == Some(false) {
        out.push(Signal {
            symbol: symbol.into(),
            kind: SignalKind::Pledge,
            priority: Priority::High,
            title: format!("{symbol}: promoter shares pledged"),
            detail: format!(
                "The shareholding filed for the quarter ended {} shows a promoter pledge; the quarter before showed none.",
                q.format("%b %Y")
            ),
            key: format!("{symbol}:pledge:{q}"),
        });
    }
    if let (Some(a), Some(b)) = (prev.promoter, last.promoter)
        && (b - a).abs() >= 1.0
    {
        out.push(Signal {
            symbol: symbol.into(),
            kind: SignalKind::Shareholding,
            priority: if b < a {
                Priority::Normal
            } else {
                Priority::Low
            },
            title: format!(
                "{symbol}: promoter stake {} {:.1} points",
                if b < a { "down" } else { "up" },
                (b - a).abs()
            ),
            detail: format!("{a:.2}% → {b:.2}% (quarter ended {}).", q.format("%b %Y")),
            key: format!("{symbol}:promoter:{q}"),
        });
    }
    let inst = |h: &crate::research::Holding| match (h.fii, h.dii) {
        (Some(f), Some(d)) => Some(f + d),
        _ => None,
    };
    if let (Some(a), Some(b)) = (inst(prev), inst(last))
        && (b - a).abs() >= 2.0
    {
        out.push(Signal {
            symbol: symbol.into(),
            kind: SignalKind::Shareholding,
            priority: Priority::Low,
            title: format!(
                "{symbol}: institutions {} {:.1} points",
                if b < a { "sold down" } else { "added" },
                (b - a).abs()
            ),
            detail: format!(
                "FII + DII {a:.2}% → {b:.2}% (quarter ended {}).",
                q.format("%b %Y")
            ),
            key: format!("{symbol}:inst:{q}"),
        });
    }
    out
}

/// Analyst upgrades and downgrades since `since` (unix seconds).
pub fn rating_signals(
    symbol: &str,
    actions: &[crate::research::AnalystAction],
    since: i64,
) -> Vec<Signal> {
    actions
        .iter()
        .filter(|a| a.ts >= since)
        .filter(|a| matches!(a.action.as_str(), "up" | "down"))
        .map(|a| {
            let up = a.action == "up";
            let target = a
                .target
                .map(|t| format!(", target {t:.2}"))
                .unwrap_or_default();
            let day = crate::events::format_date(a.ts);
            Signal {
                symbol: symbol.into(),
                kind: SignalKind::Rating,
                priority: Priority::Normal,
                title: format!(
                    "{symbol}: {} {}",
                    a.firm,
                    if up { "upgraded" } else { "downgraded" }
                ),
                detail: format!(
                    "{} → {}{target} ({day}).",
                    if a.from_grade.is_empty() {
                        "—"
                    } else {
                        &a.from_grade
                    },
                    a.to_grade
                ),
                key: format!("{symbol}:rating:{}:{}", a.ts, a.firm),
            }
        })
        .collect()
}

/// Insider (promoter, director, officer) buys and sells since `since`:
/// NSE PIT disclosures for Indian stocks, Form 4 / Yahoo for US.
pub fn insider_signals(
    symbol: &str,
    txns: &[crate::research::InsiderTxn],
    since: i64,
    currency: Option<&str>,
) -> Vec<Signal> {
    txns.iter()
        .filter(|t| t.ts >= since && t.shares > 0.0)
        .map(|t| {
            let value = if t.value > 0.0 {
                format!(
                    " worth {}",
                    crate::display::format_large_number(t.value, currency)
                )
            } else {
                String::new()
            };
            Signal {
                symbol: symbol.into(),
                kind: SignalKind::Insider,
                priority: if t.is_buy {
                    Priority::Low
                } else {
                    Priority::Normal
                },
                title: format!(
                    "{symbol}: insider {} {} shares",
                    if t.is_buy { "bought" } else { "sold" },
                    count(t.shares)
                ),
                detail: format!(
                    "{} ({}){value}, {}.",
                    t.name,
                    if t.relation.is_empty() {
                        "insider"
                    } else {
                        &t.relation
                    },
                    crate::events::format_date(t.ts)
                ),
                key: format!(
                    "{symbol}:insider:{}:{}:{}:{}",
                    t.ts,
                    if t.is_buy { "buy" } else { "sell" },
                    t.name,
                    t.shares
                ),
            }
        })
        .collect()
}

/// A share count: `12,500` or `2.5M`.
fn count(n: f64) -> String {
    if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 10_000.0 {
        format!("{:.0}K", n / 1_000.0)
    } else {
        format!("{n:.0}")
    }
}

/// Delivered signal keys, kept in `monitor_state.json`.
pub const STATE_FILE: &str = "monitor_state.json";
/// Keys kept, newest last.
const MAX_SEEN: usize = 3000;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MonitorState {
    #[serde(default)]
    pub seen: Vec<String>,
    /// Signals delivered on the last digest date, for `monitor status`.
    #[serde(default)]
    pub recent: Vec<Signal>,
    /// Unix time of the last pass.
    #[serde(default)]
    pub last_pass: Option<i64>,
    /// Session date of the last digest sent.
    #[serde(default)]
    pub last_digest: Option<String>,
    /// Where the next post-close pass resumes for the slow ownership,
    /// rating and insider checks.
    #[serde(default)]
    pub cursor: usize,
}

impl MonitorState {
    /// Signals not delivered before, without repeats.
    pub fn fresh(&self, signals: Vec<Signal>) -> Vec<Signal> {
        let mut out: Vec<Signal> = Vec::new();
        for s in signals {
            if !self.seen.contains(&s.key) && !out.iter().any(|o| o.key == s.key) {
                out.push(s);
            }
        }
        out
    }

    /// Remember delivered signals.
    pub fn record(&mut self, delivered: &[Signal]) {
        for s in delivered {
            if !self.seen.contains(&s.key) {
                self.seen.push(s.key.clone());
            }
        }
        if self.seen.len() > MAX_SEEN {
            let drop = self.seen.len() - MAX_SEEN;
            self.seen.drain(..drop);
        }
        self.recent.extend(delivered.iter().cloned());
        if self.recent.len() > 100 {
            let drop = self.recent.len() - 100;
            self.recent.drain(..drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(closes: &[f64]) -> Series {
        let start = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let mut s = Series::default();
        for (i, &c) in closes.iter().enumerate() {
            s.dates.push(start + chrono::Duration::days(i as i64));
            s.open.push(c);
            s.high.push(c * 1.005);
            s.low.push(c * 0.995);
            s.close.push(c);
            s.volume.push(1000.0);
        }
        s
    }

    /// A gently wiggling year around 100.
    fn calm(n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| 100.0 + if i % 2 == 0 { 0.3 } else { -0.3 })
            .collect()
    }

    fn kinds(v: &[Signal]) -> Vec<SignalKind> {
        v.iter().map(|s| s.kind).collect()
    }

    #[test]
    fn thresholds_read_config_and_ignore_nonsense() {
        let t = Thresholds::from_lookup(|k| match k {
            "day_drop_pct" => Some("4%".into()),
            "gap_pct" => Some("-2".into()),
            "digest" => Some("off".into()),
            _ => None,
        });
        assert_eq!(t.day_drop_pct, 4.0);
        assert_eq!(t.gap_pct, 3.0);
        assert!(!t.digest && t.enabled);
    }

    #[test]
    fn a_big_fall_is_high_priority_and_a_two_sigma_fall_normal() {
        let t = Thresholds::default();
        let mut c = calm(120);
        c.push(c[c.len() - 1] * 0.93);
        let s = price_signals("TCS.NS", &series(&c), None, &t);
        let drop = s.iter().find(|s| s.kind == SignalKind::DayDrop).unwrap();
        assert_eq!(drop.priority, Priority::High);
        assert!(drop.title.contains("7.0%"), "{}", drop.title);

        // A calm stock (σ ≈ 0.6%) falling 2.5%: under 5%, but unusual.
        let mut c = calm(120);
        c.push(c[c.len() - 1] * 0.975);
        let s = price_signals("TCS.NS", &series(&c), None, &t);
        let drop = s.iter().find(|s| s.kind == SignalKind::DayDrop).unwrap();
        assert_eq!(drop.priority, Priority::Normal);
        assert!(drop.detail.contains('σ'));

        // A 1% dip is neither.
        let mut c = calm(120);
        c.push(c[c.len() - 1] * 0.99);
        assert!(!kinds(&price_signals("X", &series(&c), None, &t)).contains(&SignalKind::DayDrop));
    }

    #[test]
    fn new_lows_cost_and_peak_falls_and_gaps() {
        let t = Thresholds::default();
        // A year falling from 200 to 140, then a close at 130.
        let mut c: Vec<f64> = (0..250).map(|i| 200.0 - i as f64 * 0.24).collect();
        c.push(130.0);
        let mut s = series(&c);
        // Space the 251 bars across ~1 calendar year of trading days so the
        // 52-week span guard (>= 330 calendar days) sees a full year.
        let start = chrono::NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        s.dates = (0..s.len())
            .map(|i| start + chrono::Duration::days((i as i64 * 3) / 2))
            .collect();
        // Opened at 137 after a 140.24 close: a 2.3% gap — under 3%.
        *s.open.last_mut().unwrap() = 137.0;
        let sig = price_signals("INFY.NS", &s, Some(180.0), &t);
        let k = kinds(&sig);
        assert!(k.contains(&SignalKind::NewLow));
        assert!(k.contains(&SignalKind::BelowCost));
        assert!(k.contains(&SignalKind::FromPeak));
        assert!(!k.contains(&SignalKind::GapDown));
        let cost = sig
            .iter()
            .find(|s| s.kind == SignalKind::BelowCost)
            .unwrap();
        // 27.8% below cost → the 20% step.
        assert!(cost.key.ends_with(":cost:20"), "{}", cost.key);
        let peak = sig.iter().find(|s| s.kind == SignalKind::FromPeak).unwrap();
        // 35% off the 200 high → the 35% step (25 + 10).
        assert!(peak.key.ends_with(":35"), "{}", peak.key);

        *s.open.last_mut().unwrap() = 135.0;
        assert!(kinds(&price_signals("INFY.NS", &s, None, &t)).contains(&SignalKind::GapDown));
    }

    #[test]
    fn buckets_step_by_ten_past_the_threshold() {
        assert_eq!(bucket(19.9, 20.0), None);
        assert_eq!(bucket(20.0, 20.0), Some(20));
        assert_eq!(bucket(29.9, 20.0), Some(20));
        assert_eq!(bucket(30.1, 20.0), Some(30));
        assert_eq!(bucket(f64::NAN, 20.0), None);
    }

    #[test]
    fn dates_tomorrow_become_signals() {
        use crate::calendar::{Event, EventKind, Source};
        let tomorrow = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        let ev = |kind, date, detail: &str| Event {
            symbol: "ITC.NS".into(),
            date,
            kind,
            detail: detail.into(),
            source: Source::Nse,
            estimate: false,
        };
        let events = vec![
            ev(EventKind::Results, tomorrow, "Financial Results"),
            ev(
                EventKind::ExDividend,
                tomorrow,
                "Interim Dividend - Rs 6.50",
            ),
            ev(EventKind::Other, tomorrow, "Record date for bonus"),
            ev(EventKind::DividendPaid, tomorrow, ""),
            ev(EventKind::Results, tomorrow + chrono::Duration::days(1), ""),
        ];
        let s = event_signals(&events, tomorrow);
        let titles: Vec<&str> = s.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "ITC.NS: results on 24 Nov",
                "ITC.NS: ex-dividend date on 24 Nov",
                "ITC.NS: record date on 24 Nov"
            ]
        );
    }

    #[test]
    fn ownership_rating_and_insider_changes() {
        use crate::research::{AnalystAction, Holding, InsiderTxn};
        let q = |m, promoter, pledged, fii, dii| Holding {
            date: NaiveDate::from_ymd_opt(2026, m, 30).unwrap(),
            promoter: Some(promoter),
            fii: Some(fii),
            dii: Some(dii),
            pledged: Some(pledged),
            ..Default::default()
        };
        let s = ownership_signals(
            "ADANI.NS",
            &[q(6, 72.0, false, 15.0, 5.0), q(9, 70.5, true, 12.0, 5.5)],
        );
        let k = kinds(&s);
        assert_eq!(
            k,
            [
                SignalKind::Pledge,
                SignalKind::Shareholding,
                SignalKind::Shareholding
            ]
        );
        assert_eq!(s[0].priority, Priority::High);
        assert!(s[1].title.contains("down 1.5 points"), "{}", s[1].title);
        assert!(ownership_signals("X", &[q(9, 70.0, false, 1.0, 1.0)]).is_empty());

        let act = |ts, action: &str| AnalystAction {
            ts,
            firm: "Acme Securities".into(),
            from_grade: "Hold".into(),
            to_grade: "Buy".into(),
            action: action.into(),
            pt_action: None,
            target: Some(1500.0),
            prior_target: None,
        };
        let r = rating_signals(
            "TCS.NS",
            &[act(2_000, "up"), act(2_000, "main"), act(10, "down")],
            1_000,
        );
        assert_eq!(r.len(), 1);
        assert!(r[0].title.contains("upgraded"));

        let txn = InsiderTxn {
            ts: 5_000,
            name: "A Promoter".into(),
            relation: "Promoter Group".into(),
            is_buy: false,
            shares: 250_000.0,
            value: 12_500_000.0,
        };
        let i = insider_signals("TCS.NS", &[txn], 1_000, Some("INR"));
        assert_eq!(i.len(), 1);
        assert!(i[0].title.contains("sold"), "{}", i[0].title);
    }

    #[test]
    fn state_sends_each_key_once() {
        let mut st = MonitorState::default();
        let sig = |key: &str| Signal {
            symbol: "X".into(),
            kind: SignalKind::DayDrop,
            priority: Priority::High,
            title: "X fell".into(),
            detail: String::new(),
            key: key.into(),
        };
        let first = st.fresh(vec![sig("a"), sig("a"), sig("b")]);
        assert_eq!(first.len(), 2);
        st.record(&first);
        assert_eq!(st.fresh(vec![sig("a"), sig("c")]).len(), 1);
    }
}
