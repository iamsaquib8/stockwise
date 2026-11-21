//! The daily digest after the close, as one message: the day's P&L, the
//! top contributors and detractors (each with the headline or filing behind
//! the move, or flagged unexplained), monitor signals, alerts that
//! triggered today, the worker's notices (LTCG, results, filings) and the
//! dates coming up.
//!
//! [`render`] is pure; the worker, `stockwise digest` and the Telegram bot
//! gather the parts.

use crate::monitor::Signal;
use crate::notify::{Message, Priority};
use crate::pulse::Pulse;
use serde::{Deserialize, Serialize};

/// A holding in the contributors list.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Line {
    pub symbol: String,
    pub change_pct: Option<f64>,
    pub pnl: Option<f64>,
    pub why: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Digest {
    /// Session date, `YYYY-MM-DD`.
    pub date: String,
    pub base_currency: String,
    pub value: Option<f64>,
    pub day_pnl: Option<f64>,
    pub day_pnl_pct: Option<f64>,
    pub up: Vec<Line>,
    pub down: Vec<Line>,
    /// Market backdrop, e.g. `NIFTY 50 +0.4%`.
    pub backdrop: Vec<String>,
    pub signals: Vec<Signal>,
    /// Alerts that triggered today.
    pub alerts: Vec<String>,
    /// Other notices (LTCG, new results, filings), batched in.
    pub notices: Vec<Message>,
    /// Dates in the next week, e.g. `Tue 24 Nov: ITC.NS results`.
    pub upcoming: Vec<String>,
}

fn line(r: &crate::pulse::Row) -> Line {
    Line {
        symbol: r.symbol.clone(),
        change_pct: r.change_pct,
        pnl: r.day_pnl,
        why: r.why.line(),
        action: r.action.clone(),
    }
}

impl Digest {
    /// The digest's numbers from a pulse; signals, alerts, notices and
    /// dates are added by the caller.
    pub fn from_pulse(p: &Pulse, date: &str, top: usize) -> Self {
        let (up, down) = p.contributors(top);
        let mut upcoming: Vec<(chrono::NaiveDate, String)> = p
            .rows
            .iter()
            .flat_map(|r| r.events.iter())
            .map(|e| {
                (
                    e.date,
                    format!(
                        "{}: {} {}",
                        e.date.format("%a %d %b"),
                        e.symbol,
                        e.kind.label().to_lowercase()
                    ),
                )
            })
            .collect();
        upcoming.sort();
        upcoming.dedup();
        Digest {
            date: date.into(),
            base_currency: p.base_currency.clone(),
            value: p.value,
            day_pnl: p.day_pnl,
            day_pnl_pct: p.day_pnl_pct,
            up: up.into_iter().map(line).collect(),
            down: down.into_iter().map(line).collect(),
            backdrop: p
                .backdrop
                .iter()
                .filter_map(|i| Some(format!("{} {:+.1}%", i.name, i.change_pct?)))
                .collect(),
            upcoming: upcoming.into_iter().map(|(_, s)| s).collect(),
            ..Default::default()
        }
    }

    /// Nothing to report: no P&L and no notices of any kind.
    pub fn is_empty(&self) -> bool {
        self.day_pnl.is_none()
            && self.signals.is_empty()
            && self.alerts.is_empty()
            && self.notices.iter().all(|n| n.title.is_empty())
    }
}

fn money(v: f64, base: &str) -> String {
    let sym = crate::market::currency_symbol(Some(base));
    let sign = if v < 0.0 { "−" } else { "+" };
    format!("{sign}{sym}{}", group(v.abs()))
}

/// Thousands separators, no decimals above 100.
pub fn group(v: f64) -> String {
    if v < 10.0 {
        return format!("{v:.2}");
    }
    let s = format!("{v:.0}");
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn pct(v: Option<f64>) -> String {
    v.map(|p| format!("{p:+.1}%")).unwrap_or_else(|| "—".into())
}

/// The digest as one message. Everything that would have been a separate
/// notice is a section here; the message takes the highest priority among
/// them.
pub fn render(d: &Digest) -> Message {
    let title = match (d.day_pnl, d.day_pnl_pct) {
        (Some(p), Some(pc)) => format!(
            "StockWise digest {}: {} ({pc:+.2}%)",
            d.date,
            money(p, &d.base_currency)
        ),
        (Some(p), None) => format!(
            "StockWise digest {}: {}",
            d.date,
            money(p, &d.base_currency)
        ),
        _ => format!("StockWise digest {}", d.date),
    };
    let mut body: Vec<String> = Vec::new();
    if let Some(v) = d.value {
        body.push(format!(
            "Portfolio {}{}",
            crate::market::currency_symbol(Some(&d.base_currency)),
            group(v)
        ));
    }
    if !d.backdrop.is_empty() {
        body.push(format!("Market: {}", d.backdrop.join(" · ")));
    }
    let mut section = |name: &str, lines: Vec<String>| {
        if !lines.is_empty() {
            body.push(String::new());
            body.push(name.to_string());
            body.extend(lines.into_iter().map(|l| format!("• {l}")));
        }
    };
    let mover = |l: &Line| {
        let mut s = format!(
            "{} {} {} — {}",
            l.symbol,
            pct(l.change_pct),
            l.pnl
                .map(|p| money(p, &d.base_currency))
                .unwrap_or_default(),
            l.why
        );
        if let Some(a) = &l.action {
            s.push_str(&format!(" [{a}]"));
        }
        s
    };
    section("Top contributors", d.up.iter().map(mover).collect());
    section("Top detractors", d.down.iter().map(mover).collect());
    section(
        "Monitor",
        d.signals
            .iter()
            .map(|s| {
                if s.detail.is_empty() {
                    s.title.clone()
                } else {
                    format!("{} — {}", s.title, s.detail)
                }
            })
            .collect(),
    );
    section("Alerts triggered", d.alerts.clone());
    section(
        "Notices",
        d.notices
            .iter()
            .filter(|n| !n.title.is_empty())
            .map(|n| {
                if n.body.is_empty() {
                    n.title.clone()
                } else {
                    format!("{} — {}", n.title, n.body.replace('\n', " "))
                }
            })
            .collect(),
    );
    section("Coming up", d.upcoming.clone());
    let priority = d
        .signals
        .iter()
        .map(|s| s.priority)
        .chain(d.notices.iter().map(|n| n.priority))
        .max()
        .unwrap_or(Priority::Normal)
        .max(Priority::Normal);
    Message::new(title, body.join("\n")).with_priority(priority)
}

// ── Triggered-alert log (for the digest) ──

pub const ALERT_LOG: &str = "alert_log.json";
const MAX_LOGGED: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Logged {
    /// Unix seconds.
    pub ts: i64,
    pub text: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AlertLog {
    pub entries: Vec<Logged>,
}

/// Record triggered alerts. Best effort: a failure only loses the digest
/// line, never the alert itself.
pub fn log_alerts(texts: &[String]) {
    if texts.is_empty() {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    let _ = crate::store::update::<AlertLog, ()>(ALERT_LOG, |log| {
        log.entries.extend(texts.iter().map(|t| Logged {
            ts: now,
            text: t.clone(),
        }));
        if log.entries.len() > MAX_LOGGED {
            let drop = log.entries.len() - MAX_LOGGED;
            log.entries.drain(..drop);
        }
        Ok(())
    });
}

/// Alerts logged at or after `since` (unix seconds).
pub fn alerts_since(since: i64) -> Vec<String> {
    crate::store::load::<AlertLog>(ALERT_LOG)
        .map(|l| {
            l.entries
                .into_iter()
                .filter(|e| e.ts >= since)
                .map(|e| e.text)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::SignalKind;

    fn digest() -> Digest {
        Digest {
            date: "2026-11-20".into(),
            base_currency: "INR".into(),
            value: Some(1_250_000.0),
            day_pnl: Some(-8_240.0),
            day_pnl_pct: Some(-0.65),
            up: vec![Line {
                symbol: "ITC.NS".into(),
                change_pct: Some(1.2),
                pnl: Some(1_100.0),
                why: "with the market (+0.4%)".into(),
                action: None,
            }],
            down: vec![Line {
                symbol: "TCS.NS".into(),
                change_pct: Some(-4.1),
                pnl: Some(-6_400.0),
                why: "unexplained — 3.2σ move, no news or filing found".into(),
                action: Some("HOLD".into()),
            }],
            backdrop: vec!["NIFTY 50 +0.4%".into()],
            signals: vec![Signal {
                symbol: "TCS.NS".into(),
                kind: SignalKind::NewLow,
                priority: Priority::Normal,
                title: "TCS.NS at a new 52-week low".into(),
                detail: "3,012.00, below the year's low of 3,050.00.".into(),
                key: "k".into(),
            }],
            alerts: vec!["INFY.NS below 1,400.00 — now ₹1,392.10".into()],
            notices: vec![
                Message::new(
                    "Lot turns long-term soon: HDFCBANK.NS",
                    "The 2025-11-27 lot ... turns long-term on 2026-11-28.",
                ),
                Message::new(
                    "New filing: ITC.NS",
                    "ITC.NS filed on 20 Nov: Outcome of Board Meeting [RESULTS].",
                ),
                Message::new("", ""),
            ],
            upcoming: vec!["Tue 24 Nov: ITC.NS ex-dividend".into()],
        }
    }

    #[test]
    fn the_digest_batches_every_notice_into_one_message() {
        let m = render(&digest());
        assert_eq!(m.title, "StockWise digest 2026-11-20: −₹8,240 (-0.65%)");
        for needle in [
            "Portfolio ₹1,250,000",
            "Market: NIFTY 50 +0.4%",
            "Top contributors\n• ITC.NS +1.2% +₹1,100 — with the market (+0.4%)",
            "Top detractors\n• TCS.NS -4.1% −₹6,400 — unexplained",
            "[HOLD]",
            "Monitor\n• TCS.NS at a new 52-week low",
            "Alerts triggered\n• INFY.NS below 1,400.00",
            "Notices\n• Lot turns long-term soon: HDFCBANK.NS",
            "• New filing: ITC.NS",
            "Coming up\n• Tue 24 Nov: ITC.NS ex-dividend",
        ] {
            assert!(m.body.contains(needle), "missing {needle:?} in\n{}", m.body);
        }
        // The silent record (empty title) is not a line.
        assert_eq!(
            m.body.matches("\n• ").count() + usize::from(m.body.starts_with("• ")),
            7
        );
    }

    #[test]
    fn an_empty_day_is_empty_and_numbers_group() {
        assert!(Digest::default().is_empty());
        assert!(!digest().is_empty());
        assert_eq!(group(1234567.0), "1,234,567");
        assert_eq!(group(999.4), "999");
        assert_eq!(group(12.5), "12");
        assert_eq!(group(7.25), "7.25");
    }
}
