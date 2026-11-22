//! Portfolio vs a benchmark index: a daily time-weighted return built from
//! what was held at each close, alpha, beta and tracking error from the
//! daily returns, and attribution by holding and by sector. Pure; `stockwise
//! portfolio benchmark` fetches the price history.
//!
//! Returns come from closing prices: a purchase or sale counts at the close
//! of its day, so the result measures the holdings, not the fills. Dividends
//! are left out on both sides (NIFTY 50 and the S&P 500 on Yahoo are price
//! indices).
//!
//! Daily contributions are linked over the period with Carino's method, so
//! the holdings' contributions add up exactly to the portfolio return and
//! their active contributions to the return over the benchmark.

use chrono::NaiveDate;
use serde::Serialize;

/// Trading days per year, to annualize daily statistics.
const DAYS_PER_YEAR: f64 = 252.0;

/// Fewer daily returns than this (about three months) and alpha/beta are noise.
pub const MIN_OBSERVATIONS: usize = 60;

/// Yearly risk-free rate for Jensen's alpha, by base currency. These are
/// round, slow-moving reference figures, not live yields:
/// - INR 7%: the 91-day Treasury bill / 10-year G-sec band of recent years;
/// - USD 4.5%: the 3-month US Treasury bill;
/// - GBP 4.5%: the Bank of England's Bank Rate area;
/// - EUR 3%: the ECB deposit-rate area.
///
/// Other currencies use 0 (alpha is then the plain excess return over beta
/// times the benchmark).
pub fn risk_free_rate(currency: &str) -> f64 {
    match currency.to_ascii_uppercase().as_str() {
        "INR" => 0.07,
        "USD" => 0.045,
        "GBP" | "GBX" => 0.045,
        "EUR" => 0.03,
        _ => 0.0,
    }
}

/// A yearly rate as a daily one over [`DAYS_PER_YEAR`] sessions.
fn daily_rate(annual: f64) -> f64 {
    (1.0 + annual).powf(1.0 / DAYS_PER_YEAR) - 1.0
}

/// One holding over the calendar of the analysis (the benchmark's trading days).
#[derive(Debug, Clone)]
pub struct HoldingDays {
    pub symbol: String,
    pub sector: String,
    /// Shares held at each day's close, in today's share count.
    pub qty: Vec<f64>,
    /// Close in the base currency on each day (forward-filled); `None`
    /// before the first price.
    pub price: Vec<Option<f64>>,
}

/// `series` (dated, ascending) on `calendar`: each day takes the last value
/// on or before it, `None` before the series starts.
pub fn align(calendar: &[NaiveDate], series: &[(NaiveDate, f64)]) -> Vec<Option<f64>> {
    let mut out = Vec::with_capacity(calendar.len());
    let mut j = 0;
    let mut last = None;
    for day in calendar {
        while j < series.len() && series[j].0 <= *day {
            if series[j].1.is_finite() && series[j].1 > 0.0 {
                last = Some(series[j].1);
            }
            j += 1;
        }
        out.push(last);
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    pub observations: usize,
    pub beta: Option<f64>,
    /// Annualized Jensen's alpha: mean daily excess return over the
    /// risk-free rate not explained by beta × the benchmark's excess return.
    pub alpha: Option<f64>,
    /// The yearly risk-free rate alpha was measured against.
    pub risk_free_rate: f64,
    /// Annualized standard deviation of the daily return difference.
    pub tracking_error: Option<f64>,
    /// Annualized excess return ÷ tracking error.
    pub information_ratio: Option<f64>,
    pub correlation: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HoldingAttribution {
    pub symbol: String,
    pub sector: String,
    /// Mean weight over the period's days (0 on days it wasn't held).
    pub avg_weight: f64,
    /// Weight at the last close.
    pub end_weight: f64,
    /// Compounded return over the days it was held.
    pub return_pct: f64,
    /// Share of the portfolio return, in percentage points; these add up to it.
    pub contribution: f64,
    /// Share of the return over the benchmark, in percentage points; these add up to it.
    pub active_contribution: f64,
    pub days_held: usize,
    /// Days it was held but had no price (left out of those days' returns).
    pub days_unpriced: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectorAttribution {
    pub sector: String,
    pub holdings: usize,
    pub avg_weight: f64,
    pub end_weight: f64,
    pub return_pct: f64,
    pub contribution: f64,
    pub active_contribution: f64,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    /// The base day (the first close with something held) and every day
    /// with a return after it.
    pub days: Vec<NaiveDate>,
    /// Growth of 100 from the base day.
    pub portfolio_index: Vec<f64>,
    pub benchmark_index: Vec<f64>,
    pub portfolio_daily: Vec<f64>,
    pub benchmark_daily: Vec<f64>,
    /// In percent.
    pub portfolio_return: f64,
    pub benchmark_return: f64,
    pub metrics: Metrics,
    pub holdings: Vec<HoldingAttribution>,
    pub sectors: Vec<SectorAttribution>,
}

impl Analysis {
    pub fn start(&self) -> NaiveDate {
        self.days[0]
    }

    pub fn end(&self) -> NaiveDate {
        *self.days.last().expect("at least two days")
    }

    /// The same analysis with alpha measured against `rf_annual` (see
    /// [`risk_free_rate`]) instead of 0.
    pub fn with_risk_free(mut self, rf_annual: f64) -> Self {
        self.metrics = metrics_rf(&self.portfolio_daily, &self.benchmark_daily, rf_annual);
        self
    }

    pub fn excess_return(&self) -> f64 {
        self.portfolio_return - self.benchmark_return
    }

    /// Annualized portfolio and benchmark returns (percent) for periods
    /// longer than about 13 months (over a year they barely differ).
    pub fn annualized(&self) -> Option<(f64, f64)> {
        let days = (self.end() - self.start()).num_days() as f64;
        if days < 400.0 {
            return None;
        }
        let a = |r: f64| ((1.0 + r / 100.0).powf(365.25 / days) - 1.0) * 100.0;
        Some((a(self.portfolio_return), a(self.benchmark_return)))
    }
}

/// Carino's linking coefficient for a period return `r`: ln(1+r)/r.
fn carino(r: f64) -> f64 {
    if r.abs() < 1e-12 || 1.0 + r <= 0.0 {
        1.0
    } else {
        (1.0 + r).ln() / r
    }
}

/// Linking coefficient for the active return `rp − rb`.
fn carino_active(rp: f64, rb: f64) -> f64 {
    if 1.0 + rp <= 0.0 || 1.0 + rb <= 0.0 {
        1.0
    } else if (rp - rb).abs() < 1e-12 {
        1.0 / (1.0 + rp)
    } else {
        ((1.0 + rp).ln() - (1.0 + rb).ln()) / (rp - rb)
    }
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Sample covariance (n − 1).
fn covariance(a: &[f64], b: &[f64]) -> f64 {
    let (ma, mb) = (mean(a), mean(b));
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - ma) * (y - mb))
        .sum::<f64>()
        / (a.len() as f64 - 1.0)
}

/// [`metrics_rf`] with a risk-free rate of 0.
pub fn metrics(rp: &[f64], rb: &[f64]) -> Metrics {
    metrics_rf(rp, rb, 0.0)
}

/// Beta, alpha against `rf_annual` (see [`risk_free_rate`]), tracking
/// error, information ratio and correlation of daily returns. Nothing but
/// the count below [`MIN_OBSERVATIONS`].
pub fn metrics_rf(rp: &[f64], rb: &[f64], rf_annual: f64) -> Metrics {
    let n = rp.len().min(rb.len());
    let (rp, rb) = (&rp[..n], &rb[..n]);
    let empty = Metrics {
        observations: n,
        beta: None,
        alpha: None,
        tracking_error: None,
        information_ratio: None,
        correlation: None,
        risk_free_rate: rf_annual,
    };
    if n < MIN_OBSERVATIONS {
        return empty;
    }
    let var_b = covariance(rb, rb);
    let var_p = covariance(rp, rp);
    let cov = covariance(rp, rb);
    let beta = (var_b > 0.0).then(|| cov / var_b);
    let rf = daily_rate(rf_annual);
    let alpha = beta.map(|b| ((mean(rp) - rf) - b * (mean(rb) - rf)) * DAYS_PER_YEAR);
    let diff: Vec<f64> = rp.iter().zip(rb).map(|(p, b)| p - b).collect();
    let te = covariance(&diff, &diff).sqrt() * DAYS_PER_YEAR.sqrt();
    let tracking_error = te.is_finite().then_some(te);
    let information_ratio = tracking_error
        .filter(|te| *te > 1e-9)
        .map(|te| mean(&diff) * DAYS_PER_YEAR / te);
    let correlation = (var_p > 0.0 && var_b > 0.0).then(|| cov / (var_p.sqrt() * var_b.sqrt()));
    Metrics {
        observations: n,
        beta,
        alpha,
        tracking_error,
        information_ratio,
        correlation,
        risk_free_rate: rf_annual,
    }
}

/// One day's return: the benchmark's, and each held, priced holding's
/// weight (at the previous close) and return.
struct Day {
    rp: f64,
    rb: f64,
    legs: Vec<(usize, f64, f64)>,
}

/// Compare the portfolio described by `holdings` with `benchmark` (closes in
/// the base currency) on `calendar`. The period starts at the first close
/// where something priced is held. `None` when there is no day with a return.
pub fn analyze(
    calendar: &[NaiveDate],
    benchmark: &[Option<f64>],
    holdings: &[HoldingDays],
) -> Option<Analysis> {
    let n = calendar.len();
    let value = |h: &HoldingDays, t: usize| match h.price[t] {
        Some(p) if h.qty[t] > 0.0 => h.qty[t] * p,
        _ => 0.0,
    };
    let mut days: Vec<NaiveDate> = Vec::new();
    let mut recorded: Vec<Day> = Vec::new();
    let mut unpriced = vec![0usize; holdings.len()];
    for t in 1..n {
        let (Some(b0), Some(b1)) = (benchmark[t - 1], benchmark[t]) else {
            continue;
        };
        // Holdings held at the previous close with a price on both days.
        let mut legs = Vec::new();
        let mut missing = Vec::new();
        let mut v_prev = 0.0;
        for (i, h) in holdings.iter().enumerate() {
            if h.qty[t - 1] <= 0.0 {
                continue;
            }
            match (h.price[t - 1], h.price[t]) {
                (Some(p0), Some(p1)) => {
                    let v = h.qty[t - 1] * p0;
                    v_prev += v;
                    legs.push((i, v, p1 / p0 - 1.0));
                }
                _ => missing.push(i),
            }
        }
        if v_prev <= 0.0 {
            continue;
        }
        for i in missing {
            unpriced[i] += 1;
        }
        if days.is_empty() {
            days.push(calendar[t - 1]);
        }
        let legs: Vec<(usize, f64, f64)> = legs
            .into_iter()
            .map(|(i, v, r)| (i, v / v_prev, r))
            .collect();
        let rp = legs.iter().map(|(_, w, r)| w * r).sum();
        days.push(calendar[t]);
        recorded.push(Day {
            rp,
            rb: b1 / b0 - 1.0,
            legs,
        });
    }
    if recorded.is_empty() {
        return None;
    }

    let portfolio_daily: Vec<f64> = recorded.iter().map(|d| d.rp).collect();
    let benchmark_daily: Vec<f64> = recorded.iter().map(|d| d.rb).collect();
    let index = |rs: &[f64]| -> Vec<f64> {
        let mut v = vec![100.0];
        for r in rs {
            v.push(v.last().expect("starts at 100") * (1.0 + r));
        }
        v
    };
    let portfolio_index = index(&portfolio_daily);
    let benchmark_index = index(&benchmark_daily);
    let rp_total = portfolio_index.last().expect("non-empty") / 100.0 - 1.0;
    let rb_total = benchmark_index.last().expect("non-empty") / 100.0 - 1.0;
    let k_total = carino(rp_total);
    let ka_total = carino_active(rp_total, rb_total);

    let count = recorded.len() as f64;
    let mut contribution = vec![0.0; holdings.len()];
    let mut active = vec![0.0; holdings.len()];
    let mut weight_sum = vec![0.0; holdings.len()];
    let mut growth = vec![1.0; holdings.len()];
    let mut held = vec![0usize; holdings.len()];
    for d in &recorded {
        let k = carino(d.rp) / k_total;
        let ka = carino_active(d.rp, d.rb) / ka_total;
        for &(i, w, r) in &d.legs {
            contribution[i] += k * w * r;
            active[i] += ka * w * (r - d.rb);
            weight_sum[i] += w;
            growth[i] *= 1.0 + r;
            held[i] += 1;
        }
    }
    let last = n - 1;
    let v_last: f64 = holdings.iter().map(|h| value(h, last)).sum();
    let end_weight = |h: &HoldingDays| {
        if v_last > 0.0 {
            value(h, last) / v_last
        } else {
            0.0
        }
    };
    let mut rows: Vec<HoldingAttribution> = holdings
        .iter()
        .enumerate()
        .filter(|(i, _)| held[*i] > 0 || unpriced[*i] > 0)
        .map(|(i, h)| HoldingAttribution {
            symbol: h.symbol.clone(),
            sector: h.sector.clone(),
            avg_weight: weight_sum[i] / count * 100.0,
            end_weight: end_weight(h) * 100.0,
            return_pct: (growth[i] - 1.0) * 100.0,
            contribution: contribution[i] * 100.0,
            active_contribution: active[i] * 100.0,
            days_held: held[i],
            days_unpriced: unpriced[i],
        })
        .collect();
    rows.sort_by(|a, b| b.contribution.total_cmp(&a.contribution));

    // Sectors: the same sums, and a return compounded from each day's
    // weight-averaged return of the sector's holdings.
    let mut names: Vec<String> = rows.iter().map(|r| r.sector.clone()).collect();
    names.sort();
    names.dedup();
    let mut sectors: Vec<SectorAttribution> = names
        .into_iter()
        .map(|sector| {
            let members: Vec<&HoldingAttribution> =
                rows.iter().filter(|r| r.sector == sector).collect();
            let mut g = 1.0;
            for d in &recorded {
                let (w, wr) = d
                    .legs
                    .iter()
                    .filter(|(i, _, _)| holdings[*i].sector == sector)
                    .fold((0.0, 0.0), |(w, wr), (_, wi, ri)| (w + wi, wr + wi * ri));
                if w > 0.0 {
                    g *= 1.0 + wr / w;
                }
            }
            SectorAttribution {
                holdings: members.len(),
                avg_weight: members.iter().map(|r| r.avg_weight).sum(),
                end_weight: members.iter().map(|r| r.end_weight).sum(),
                return_pct: (g - 1.0) * 100.0,
                contribution: members.iter().map(|r| r.contribution).sum(),
                active_contribution: members.iter().map(|r| r.active_contribution).sum(),
                sector,
            }
        })
        .collect();
    sectors.sort_by(|a, b| b.contribution.total_cmp(&a.contribution));

    Some(Analysis {
        metrics: metrics(&portfolio_daily, &benchmark_daily),
        days,
        portfolio_index,
        benchmark_index,
        portfolio_daily,
        benchmark_daily,
        portfolio_return: rp_total * 100.0,
        benchmark_return: rb_total * 100.0,
        holdings: rows,
        sectors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal(n: usize) -> Vec<NaiveDate> {
        let start = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        (0..n)
            .map(|i| start + chrono::Days::new(i as u64))
            .collect()
    }

    fn h(symbol: &str, sector: &str, qty: Vec<f64>, price: Vec<f64>) -> HoldingDays {
        HoldingDays {
            symbol: symbol.into(),
            sector: sector.into(),
            qty,
            price: price.into_iter().map(Some).collect(),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn align_forward_fills_and_starts_empty() {
        let c = cal(5);
        let s = vec![(c[1], 10.0), (c[3], 12.0)];
        assert_eq!(
            align(&c, &s),
            vec![None, Some(10.0), Some(10.0), Some(12.0), Some(12.0)]
        );
    }

    #[test]
    fn two_holdings_worked_example() {
        // Day 0 → 1: A (100 sh, ₹10 → ₹11, +10%) and B (50 sh, ₹20 → ₹19,
        // −5%), equal ₹1,000 weights: portfolio +2.5%, benchmark +1%.
        // Day 1 → 2: A ₹11 → ₹11.55 (+5%), B flat; weights at day-1 close
        // are 1,100/2,050 and 950/2,050: portfolio +2.6829%, benchmark −1%.
        let c = cal(3);
        let holdings = vec![
            h("A", "IT", vec![100.0; 3], vec![10.0, 11.0, 11.55]),
            h("B", "Banks", vec![50.0; 3], vec![20.0, 19.0, 19.0]),
        ];
        let bench = vec![Some(100.0), Some(101.0), Some(99.99)];
        let a = analyze(&c, &bench, &holdings).unwrap();
        // Value 2,000 → 2,105: TWR = 5.25% with no trades.
        assert!(close(a.portfolio_return, 5.25), "{}", a.portfolio_return);
        assert!(close(a.benchmark_return, -0.01));
        assert_eq!(a.days, c);
        // Contributions add up to the portfolio return, active ones to the excess.
        let sum: f64 = a.holdings.iter().map(|r| r.contribution).sum();
        let act: f64 = a.holdings.iter().map(|r| r.active_contribution).sum();
        assert!(close(sum, a.portfolio_return), "{sum}");
        assert!(close(act, a.excess_return()), "{act}");
        let ha = a.holdings.iter().find(|r| r.symbol == "A").unwrap();
        assert!(close(ha.return_pct, 15.5));
        assert!(close(ha.end_weight, 1155.0 / 2105.0 * 100.0));
        assert!(ha.contribution > 0.0 && a.holdings[0].symbol == "A");
        let sb = a.sectors.iter().find(|s| s.sector == "Banks").unwrap();
        assert!(close(sb.return_pct, -5.0));
        assert_eq!(a.metrics.observations, 2);
        assert!(a.metrics.beta.is_none(), "too few days for beta");
    }

    #[test]
    fn a_purchase_counts_from_its_close() {
        // Nothing held on day 0; 10 shares bought on day 1 (counted from day 1's
        // close), price 100 → 110 on day 2. The period starts on day 1.
        let c = cal(3);
        let holdings = vec![h(
            "A",
            "IT",
            vec![0.0, 10.0, 10.0],
            vec![90.0, 100.0, 110.0],
        )];
        let bench = vec![Some(1.0), Some(1.0), Some(1.05)];
        let a = analyze(&c, &bench, &holdings).unwrap();
        assert_eq!(a.days, c[1..].to_vec());
        assert!(close(a.portfolio_return, 10.0));
        assert!(close(a.benchmark_return, 5.0));
        // Adding shares changes the weight, not the return.
        let holdings = vec![
            h(
                "A",
                "IT",
                vec![10.0, 10.0, 30.0, 30.0],
                vec![100.0, 110.0, 121.0, 121.0],
            ),
            h(
                "B",
                "IT",
                vec![10.0, 10.0, 10.0, 10.0],
                vec![100.0, 100.0, 100.0, 110.0],
            ),
        ];
        let c = cal(4);
        let bench = vec![Some(1.0); 4];
        let a = analyze(&c, &bench, &holdings).unwrap();
        // Day 1: +5%. Day 2: A is 1100/2100 of the portfolio → +5.238%.
        // Day 3: B is 1000/(3630+1000) → +2.160%.
        let expected = 1.05 * (1.0 + 0.1 * 1100.0 / 2100.0) * (1.0 + 0.1 * 1000.0 / 4630.0);
        assert!(close(a.portfolio_return, (expected - 1.0) * 100.0));
    }

    #[test]
    fn unpriced_days_are_counted_and_left_out() {
        let c = cal(3);
        let holdings = vec![
            h("A", "IT", vec![1.0; 3], vec![100.0, 110.0, 121.0]),
            HoldingDays {
                symbol: "NEW".into(),
                sector: "Unknown".into(),
                qty: vec![1.0; 3],
                price: vec![None, None, Some(50.0)],
            },
        ];
        let a = analyze(&c, &[Some(1.0); 3], &holdings).unwrap();
        assert!(close(a.portfolio_return, 21.0));
        let new = a.holdings.iter().find(|r| r.symbol == "NEW").unwrap();
        assert_eq!((new.days_held, new.days_unpriced), (0, 2));
    }

    #[test]
    fn beta_alpha_and_tracking_error() {
        // The portfolio moves exactly twice the benchmark each day: beta 2,
        // alpha 0, correlation 1.
        let rb: Vec<f64> = (0..60).map(|i| ((i as f64) * 0.7).sin() * 0.01).collect();
        let rp: Vec<f64> = rb.iter().map(|r| 2.0 * r).collect();
        let m = metrics(&rp, &rb);
        assert!(close(m.beta.unwrap(), 2.0));
        assert!(m.alpha.unwrap().abs() < 1e-12);
        assert!(close(m.correlation.unwrap(), 1.0));
        // Tracking error = sd(rp − rb) × √252 = sd(rb) × √252.
        let sd = covariance(&rb, &rb).sqrt();
        assert!(close(m.tracking_error.unwrap(), sd * 252f64.sqrt()));
        // A constant 0.1% a day on top: alpha 0.1% × 252 = 25.2%.
        let rp: Vec<f64> = rb.iter().map(|r| r + 0.001).collect();
        let m = metrics(&rp, &rb);
        assert!(close(m.beta.unwrap(), 1.0));
        assert!(close(m.alpha.unwrap(), 0.252));
        assert!(m.tracking_error.unwrap() < 1e-9);
        assert!(m.information_ratio.is_none());
        assert!(metrics(&rp[..5], &rb[..5]).beta.is_none());
        // 59 observations are too few.
        assert!(metrics(&rp[..59], &rb[..59]).alpha.is_none());
    }

    #[test]
    fn alpha_is_measured_over_the_risk_free_rate() {
        // A beta-1 portfolio with no excess return has 0 alpha at any rate.
        let rb: Vec<f64> = (0..80).map(|i| ((i as f64) * 0.7).sin() * 0.01).collect();
        let m = metrics_rf(&rb, &rb, 0.07);
        assert!(m.alpha.unwrap().abs() < 1e-12);
        assert_eq!(m.risk_free_rate, 0.07);
        // Beta 2: Jensen's alpha adds (β − 1) × rf_daily × 252 to the plain
        // figure (0 here): leverage on the benchmark is not skill.
        let rp: Vec<f64> = rb.iter().map(|r| 2.0 * r).collect();
        let plain = metrics(&rp, &rb).alpha.unwrap();
        let jensen = metrics_rf(&rp, &rb, 0.07).alpha.unwrap();
        let rf_d = 1.07f64.powf(1.0 / 252.0) - 1.0;
        assert!(close(jensen, plain + rf_d * 252.0));
        assert!(jensen > plain);
        assert_eq!(risk_free_rate("INR"), 0.07);
        assert_eq!(risk_free_rate("usd"), 0.045);
        assert_eq!(risk_free_rate("JPY"), 0.0);
    }

    #[test]
    fn carino_linking_handles_flat_days() {
        assert!(close(carino(0.0), 1.0));
        assert!(close(carino_active(0.02, 0.02), 1.0 / 1.02));
        assert!(close(carino(0.1), 1.1f64.ln() / 0.1));
    }

    #[test]
    fn nothing_held_means_no_analysis() {
        let c = cal(3);
        let holdings = vec![h("A", "IT", vec![0.0; 3], vec![1.0; 3])];
        assert!(analyze(&c, &[Some(1.0); 3], &holdings).is_none());
    }
}
