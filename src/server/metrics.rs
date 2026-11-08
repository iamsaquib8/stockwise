//! Prometheus text exposition for `/metrics`, written by hand: a handful of
//! counters and one histogram don't need a metrics crate.

use crate::daemon_status::DaemonStatus;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Request latency buckets, in seconds.
const BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

#[derive(Default)]
struct Histogram {
    /// Cumulative counts per bucket in [`BUCKETS`].
    buckets: [u64; BUCKETS.len()],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, secs: f64) {
        for (i, le) in BUCKETS.iter().enumerate() {
            if secs <= *le {
                self.buckets[i] += 1;
            }
        }
        self.sum += secs;
        self.count += 1;
    }
}

#[derive(Default)]
struct Registry {
    /// (method, route, status) → count
    requests: BTreeMap<(String, String, u16), u64>,
    /// route → latency
    latency: BTreeMap<String, Histogram>,
}

pub struct Metrics {
    started: Instant,
    registry: Mutex<Registry>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Escape a label value (backslash, quote, newline).
fn label(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn header(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            registry: Mutex::new(Registry::default()),
        }
    }

    /// Record one served request. `route` is the matched route template
    /// (`/api/history/{symbol}`), never the raw path, to bound cardinality.
    pub fn observe(&self, method: &str, route: &str, status: u16, elapsed: Duration) {
        let mut reg = self.registry.lock().unwrap();
        *reg.requests
            .entry((method.to_string(), route.to_string(), status))
            .or_default() += 1;
        reg.latency
            .entry(route.to_string())
            .or_default()
            .observe(elapsed.as_secs_f64());
    }

    /// The full exposition. `daemons` are the intraday daemons' last saved
    /// states (one per market that has run here); `now` is Unix seconds.
    pub fn render(&self, daemons: &[DaemonStatus], now: i64) -> String {
        let mut out = String::new();

        header(
            &mut out,
            "stockwise_build_info",
            "gauge",
            "StockWise version.",
        );
        let _ = writeln!(
            out,
            "stockwise_build_info{{version=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION")
        );
        header(
            &mut out,
            "stockwise_uptime_seconds",
            "gauge",
            "Seconds since the server started.",
        );
        let _ = writeln!(
            out,
            "stockwise_uptime_seconds {:.3}",
            self.started.elapsed().as_secs_f64()
        );

        {
            let reg = self.registry.lock().unwrap();
            header(
                &mut out,
                "stockwise_http_requests_total",
                "counter",
                "HTTP requests served, by method, route and status.",
            );
            for ((method, route, status), n) in &reg.requests {
                let _ = writeln!(
                    out,
                    "stockwise_http_requests_total{{method=\"{}\",route=\"{}\",status=\"{}\"}} {}",
                    label(method),
                    label(route),
                    status,
                    n
                );
            }
            header(
                &mut out,
                "stockwise_http_request_duration_seconds",
                "histogram",
                "HTTP request latency, by route.",
            );
            for (route, h) in &reg.latency {
                let route = label(route);
                for (le, n) in BUCKETS.iter().zip(h.buckets) {
                    let _ = writeln!(
                        out,
                        "stockwise_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"{le}\"}} {n}"
                    );
                }
                let _ = writeln!(
                    out,
                    "stockwise_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"+Inf\"}} {}",
                    h.count
                );
                let _ = writeln!(
                    out,
                    "stockwise_http_request_duration_seconds_sum{{route=\"{route}\"}} {}",
                    h.sum
                );
                let _ = writeln!(
                    out,
                    "stockwise_http_request_duration_seconds_count{{route=\"{route}\"}} {}",
                    h.count
                );
            }
        }

        let stats = &crate::api::STATS;
        let load = |c: &std::sync::atomic::AtomicU64| c.load(Ordering::Relaxed);
        header(
            &mut out,
            "stockwise_upstream_requests_total",
            "counter",
            "HTTP requests sent to Yahoo Finance, retries included.",
        );
        let _ = writeln!(
            out,
            "stockwise_upstream_requests_total {}",
            load(&stats.requests)
        );
        header(
            &mut out,
            "stockwise_upstream_errors_total",
            "counter",
            "Yahoo Finance fetches that failed after all retries.",
        );
        let _ = writeln!(
            out,
            "stockwise_upstream_errors_total {}",
            load(&stats.errors)
        );
        header(
            &mut out,
            "stockwise_cache_hits_total",
            "counter",
            "Market-data cache hits, by layer.",
        );
        let _ = writeln!(
            out,
            "stockwise_cache_hits_total{{layer=\"redis\"}} {}",
            load(&stats.cache_hits_redis)
        );
        let _ = writeln!(
            out,
            "stockwise_cache_hits_total{{layer=\"sqlite\"}} {}",
            load(&stats.cache_hits_sqlite)
        );
        let _ = writeln!(
            out,
            "stockwise_cache_hits_total{{layer=\"memory\"}} {}",
            load(&stats.cache_hits_memory)
        );
        header(
            &mut out,
            "stockwise_cache_misses_total",
            "counter",
            "Market-data cache misses.",
        );
        let _ = writeln!(
            out,
            "stockwise_cache_misses_total {}",
            load(&stats.cache_misses)
        );

        header(
            &mut out,
            "stockwise_daemon_up",
            "gauge",
            "1 if the market's intraday daemon saved its state in the last 45 minutes.",
        );
        // India and the US always; a global exchange once its daemon has run.
        let mut markets: Vec<&str> = crate::daemon_status::MARKETS.to_vec();
        for d in daemons {
            if !markets.contains(&d.market.as_str()) {
                markets.push(&d.market);
            }
        }
        for market in markets {
            let fresh = daemons
                .iter()
                .any(|d| d.market == market && d.is_fresh(now));
            let _ = writeln!(
                out,
                "stockwise_daemon_up{{market=\"{}\"}} {}",
                market,
                u8::from(fresh)
            );
        }
        // (name, help, money, session-scoped, value). Session-scoped gauges
        // describe one day of paper trading and are omitted once the daemon's
        // saved date is no longer the current session, so yesterday's open
        // positions do not pose as today's.
        let gauges: [(&str, &str, bool, bool, fn(&DaemonStatus, i64) -> f64); 9] = [
            (
                "stockwise_daemon_status_age_seconds",
                "Seconds since the daemon last saved its state.",
                false,
                false,
                |d, now| d.age_secs(now) as f64,
            ),
            (
                "stockwise_daemon_last_update_timestamp_seconds",
                "Unix time of the daemon's last state save; use time() - this for the age.",
                false,
                false,
                |d, _| d.updated_at as f64,
            ),
            (
                "stockwise_daemon_open_positions",
                "Open paper positions in the current session.",
                false,
                true,
                |d, _| d.open_positions as f64,
            ),
            (
                "stockwise_daemon_day_pnl",
                "Day P&L of the current paper session (realized + unrealized).",
                true,
                true,
                |d, _| d.day_pnl,
            ),
            (
                "stockwise_daemon_realized_pnl",
                "Realized P&L of the current paper session.",
                true,
                true,
                |d, _| d.realized_pnl,
            ),
            (
                "stockwise_daemon_unrealized_pnl",
                "Unrealized P&L of open paper positions in the current session.",
                true,
                true,
                |d, _| d.unrealized_pnl,
            ),
            (
                "stockwise_daemon_capital",
                "Paper capital the daemon trades with.",
                true,
                false,
                |d, _| d.capital,
            ),
            (
                "stockwise_daemon_kill_switch",
                "1 once the current session's daily loss limit tripped (no new entries today).",
                false,
                true,
                |d, _| if d.kill_switch { 1.0 } else { 0.0 },
            ),
            (
                "stockwise_daemon_settled",
                "1 once the current session is over and recorded in the simulator history.",
                false,
                true,
                |d, _| if d.settled { 1.0 } else { 0.0 },
            ),
        ];
        if !daemons.is_empty() {
            for (name, help, money, session, value) in gauges {
                let shown: Vec<&DaemonStatus> = daemons
                    .iter()
                    .filter(|d| !session || d.is_current_session(now))
                    .collect();
                if shown.is_empty() {
                    continue;
                }
                header(&mut out, name, "gauge", help);
                for d in shown {
                    // Money is in the market's own currency: INR for IN, USD
                    // for US. The label keeps a sum() across markets honest.
                    if money {
                        let _ = writeln!(
                            out,
                            "{name}{{market=\"{}\",currency=\"{}\"}} {}",
                            label(&d.market),
                            label(&d.currency),
                            value(d, now)
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "{name}{{market=\"{}\"}} {}",
                            label(&d.market),
                            value(d, now)
                        );
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_are_cumulative() {
        let m = Metrics::new();
        m.observe("GET", "/api/quote", 200, Duration::from_millis(3));
        m.observe("GET", "/api/quote", 200, Duration::from_millis(300));
        m.observe("GET", "/api/quote", 404, Duration::from_secs(20));
        let text = m.render(&[], 0);
        assert!(text.contains(
            "stockwise_http_requests_total{method=\"GET\",route=\"/api/quote\",status=\"200\"} 2"
        ));
        assert!(text.contains(
            "stockwise_http_requests_total{method=\"GET\",route=\"/api/quote\",status=\"404\"} 1"
        ));
        assert!(text.contains(
            "stockwise_http_request_duration_seconds_bucket{route=\"/api/quote\",le=\"0.005\"} 1"
        ));
        assert!(text.contains(
            "stockwise_http_request_duration_seconds_bucket{route=\"/api/quote\",le=\"0.5\"} 2"
        ));
        assert!(text.contains(
            "stockwise_http_request_duration_seconds_bucket{route=\"/api/quote\",le=\"10\"} 2"
        ));
        assert!(text.contains(
            "stockwise_http_request_duration_seconds_bucket{route=\"/api/quote\",le=\"+Inf\"} 3"
        ));
        assert!(
            text.contains("stockwise_http_request_duration_seconds_count{route=\"/api/quote\"} 3")
        );
        assert!(text.contains("stockwise_daemon_up{market=\"IN\"} 0"));
        assert!(text.contains("stockwise_daemon_up{market=\"US\"} 0"));
        assert!(!text.contains("stockwise_daemon_open_positions"));
    }

    #[test]
    fn every_sample_has_help_and_type() {
        let d = DaemonStatus {
            market: "US".into(),
            // `now` is 160, which is still 1969-12-31 in New York.
            date: "1969-12-31".into(),
            updated_at: 100,
            open_positions: 2,
            currency: "USD".into(),
            day_pnl: -12.5,
            kill_switch: true,
            ..Default::default()
        };
        let m = Metrics::new();
        m.observe("GET", "/healthz", 200, Duration::from_millis(1));
        let text = m.render(&[d], 160);
        assert!(text.contains("stockwise_daemon_up{market=\"US\"} 1"));
        assert!(text.contains("stockwise_daemon_up{market=\"IN\"} 0"));
        assert!(text.contains("stockwise_daemon_open_positions{market=\"US\"} 2"));
        assert!(text.contains("stockwise_daemon_day_pnl{market=\"US\",currency=\"USD\"} -12.5"));
        assert!(text.contains("stockwise_daemon_last_update_timestamp_seconds{market=\"US\"} 100"));
        assert!(text.contains("stockwise_daemon_kill_switch{market=\"US\"} 1"));
        assert!(text.contains("stockwise_daemon_status_age_seconds{market=\"US\"} 60"));
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let name = line
                .split(['{', ' '])
                .next()
                .unwrap()
                .trim_end_matches("_bucket")
                .trim_end_matches("_sum")
                .trim_end_matches("_count");
            assert!(
                text.contains(&format!("# TYPE {name} ")),
                "no TYPE for {line}"
            );
        }
    }

    #[test]
    fn settled_day_exports_no_open_positions() {
        // A daemon that missed the square-off saves a settled state whose
        // rows are still `Open`; the gauges must not call them open.
        let json = r#"{"date": "2026-10-21", "market": "IN", "capital": 25000.0, "settled": true,
            "positions": [{"status": "Open", "pnl": 30.0}, {"status": "T1Hit", "pnl": 20.0}],
            "risk": {"realized_pnl": 0.0, "killed": false},
            "updated_at": "2026-10-21T05:30:00Z"}"#;
        let d = DaemonStatus::from_state_json(json).unwrap();
        let text = Metrics::new().render(&[d], 1_792_560_660);
        assert!(text.contains("stockwise_daemon_open_positions{market=\"IN\"} 0"));
        assert!(text.contains("stockwise_daemon_unrealized_pnl{market=\"IN\",currency=\"INR\"} 0"));
        assert!(text.contains("stockwise_daemon_settled{market=\"IN\"} 1"));
        assert!(
            text.contains(
                "stockwise_daemon_last_update_timestamp_seconds{market=\"IN\"} 1792560600"
            )
        );
    }

    #[test]
    fn yesterdays_session_gauges_do_not_outlive_the_date() {
        let json = r#"{"date": "2026-10-21", "market": "IN", "capital": 25000.0,
            "positions": [{"status": "Open", "pnl": 30.0}],
            "risk": {"realized_pnl": 0.0, "killed": true},
            "updated_at": "2026-10-21T05:30:00Z"}"#;
        let d = DaemonStatus::from_state_json(json).unwrap();
        let next_day = 1_792_560_600 + 24 * 3600;
        let text = Metrics::new().render(std::slice::from_ref(&d), next_day);
        for name in [
            "stockwise_daemon_open_positions",
            "stockwise_daemon_day_pnl",
            "stockwise_daemon_realized_pnl",
            "stockwise_daemon_unrealized_pnl",
            "stockwise_daemon_kill_switch",
            "stockwise_daemon_settled",
        ] {
            assert!(
                !text.contains(name),
                "{name} survived the session date:\n{text}"
            );
        }
        // The health and age signals stay, so the crash is still visible.
        assert!(text.contains("stockwise_daemon_up{market=\"IN\"} 0"));
        assert!(text.contains("stockwise_daemon_status_age_seconds{market=\"IN\"} 86400"));
        assert!(text.contains("stockwise_daemon_last_update_timestamp_seconds{market=\"IN\"} "));
        assert!(text.contains("stockwise_daemon_capital{market=\"IN\",currency=\"INR\"} 25000"));
    }

    #[test]
    fn a_stale_daemon_still_shows_the_current_sessions_last_numbers() {
        // Same date but no save for hours: those are the crash's last numbers,
        // not yesterday's, so they stay until the date turns over.
        let d = DaemonStatus {
            market: "IN".into(),
            date: "2026-10-21".into(),
            updated_at: 1_792_560_600,
            open_positions: 2,
            ..Default::default()
        };
        let text = Metrics::new().render(&[d], 1_792_560_600 + 5 * 3600);
        assert!(text.contains("stockwise_daemon_up{market=\"IN\"} 0"));
        assert!(text.contains("stockwise_daemon_open_positions{market=\"IN\"} 2"));
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
