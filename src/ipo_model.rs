//! The listing-day model for Indian mainboard IPOs: expected listing-day
//! return from the final subscription and the IPO market's recent listings,
//! fitted on past NSE mainboard issues.
//!
//! ```text
//! listing return % = b0 + b1·ln(1+QIB x) + b2·ln(1+NII x) + b3·ln(1+retail x) + b4·regime %
//! ```
//!
//! `regime %` is the recency-weighted median listing-day return of the
//! mainboard issues listed in the 180 days before (at least
//! [`REGIME_MIN_SAMPLE`] of them; otherwise the training mean is used and
//! the gap counts against coverage). The probability of listing below the
//! issue price comes from the fitted residuals, not from a normal curve.
//!
//! SME issues are not in the fit: NSE Emerge and BSE SME list with a
//! different price discovery (a 90% cap over the issue price in the
//! pre-open session) and are gated separately (`crate::ipo_verdict`).
//!
//! The shipped coefficients are [`builtin`]; `stockwise ipo calibrate`
//! refits them on the user's own data and saves `ipo_model.json` in the data
//! directory, which then takes precedence. docs/methodology.md ("IPO
//! verdicts") records the fit.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Listings that feed the regime: the last 180 days before the date.
pub const REGIME_WINDOW_DAYS: i64 = 180;
/// Weight halves every 45 days of age.
pub const REGIME_HALF_LIFE_DAYS: f64 = 45.0;
/// Fewer listings than this in the window: no regime reading.
pub const REGIME_MIN_SAMPLE: usize = 8;
/// Huber tuning constant (95% efficiency under normal errors).
const HUBER_K: f64 = 1.345;
/// Residual quantiles stored with the model: 0%, 5%, ..., 100%.
pub const QUANTILES: usize = 21;

/// What the model reads for one issue.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Features {
    pub qib_x: f64,
    pub nii_x: f64,
    pub retail_x: f64,
    /// Recency-weighted median listing-day return of recent listings, in
    /// percent; `None` when fewer than [`REGIME_MIN_SAMPLE`] listings.
    pub regime_pct: Option<f64>,
}

/// A fitted listing-day model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListingModel {
    pub intercept: f64,
    pub b_qib: f64,
    pub b_nii: f64,
    pub b_retail: f64,
    pub b_regime: f64,
    /// Mean regime of the training rows, used when the regime is unknown.
    pub regime_mean: f64,
    /// `YYYY-MM-DD` of the fit.
    pub fit_date: String,
    /// Issues in the fit.
    pub n: usize,
    /// Listing dates covered by the fit (`YYYY-MM-DD`).
    pub sample_from: String,
    pub sample_to: String,
    /// R² of the fit (unweighted residuals).
    pub r2: f64,
    /// Residual standard deviation, percentage points.
    pub resid_sd: f64,
    /// Residual quantiles at 0%, 5%, ..., 100%, percentage points.
    pub residual_quantiles: Vec<f64>,
    /// `ols` or `huber`.
    pub method: String,
}

impl ListingModel {
    /// Expected listing-day return, percent over the issue price.
    pub fn predict(&self, f: &Features) -> f64 {
        self.intercept
            + self.b_qib * (1.0 + f.qib_x.max(0.0)).ln()
            + self.b_nii * (1.0 + f.nii_x.max(0.0)).ln()
            + self.b_retail * (1.0 + f.retail_x.max(0.0)).ln()
            + self.b_regime * f.regime_pct.unwrap_or(self.regime_mean)
    }

    /// Probability that the listing-day close is below the issue price when
    /// the model expects `mean_pct`: the share of fitted residuals below
    /// `-mean_pct`, read off the stored quantiles.
    pub fn p_loss(&self, mean_pct: f64) -> f64 {
        residual_cdf(&self.residual_quantiles, -mean_pct)
    }

    /// The listing-day return at residual quantile `q` (0 to 1).
    pub fn quantile(&self, mean_pct: f64, q: f64) -> f64 {
        mean_pct + quantile_of(&self.residual_quantiles, q)
    }

    /// The smallest QIB multiple, with the other inputs held, at which
    /// `ok(expected %, P(loss))` holds; `None` when no multiple up to
    /// 1,000x does (or the model gives QIB demand no positive weight).
    pub fn qib_threshold(&self, f: &Features, ok: impl Fn(f64, f64) -> bool) -> Option<f64> {
        let at = |q: f64| {
            let m = self.predict(&Features { qib_x: q, ..*f });
            ok(m, self.p_loss(m))
        };
        if self.b_qib <= 0.0 {
            return None;
        }
        if at(0.0) {
            return Some(0.0);
        }
        let (mut lo, mut hi) = (0.0f64, 1000.0f64);
        if !at(hi) {
            return None;
        }
        // The prediction rises with ln(1+q): bisect on that scale.
        for _ in 0..60 {
            let mid = ((1.0 + lo).ln() + (1.0 + hi).ln()) / 2.0;
            let q = mid.exp() - 1.0;
            if at(q) {
                hi = q;
            } else {
                lo = q;
            }
        }
        Some(hi)
    }
}

/// The model shipped with this release, or `None` while no fit has been
/// recorded. See docs/methodology.md ("IPO verdicts").
pub fn builtin() -> Option<ListingModel> {
    BUILTIN.map(|b| ListingModel {
        intercept: b.0[0],
        b_qib: b.0[1],
        b_nii: b.0[2],
        b_retail: b.0[3],
        b_regime: b.0[4],
        regime_mean: b.1,
        fit_date: b.2.to_string(),
        n: b.3,
        sample_from: b.4.to_string(),
        sample_to: b.5.to_string(),
        r2: b.6,
        resid_sd: b.7,
        residual_quantiles: b.8.to_vec(),
        method: "huber".to_string(),
    })
}

/// (coefficients b0..b4, regime mean, fit date, n, from, to, R², residual
/// sd, residual quantiles). Written by `stockwise ipo calibrate --print-const`.
type Builtin = (
    [f64; 5],
    f64,
    &'static str,
    usize,
    &'static str,
    &'static str,
    f64,
    f64,
    [f64; QUANTILES],
);
const BUILTIN: Option<Builtin> = None;

/// The model in use: the user's `ipo_model.json` when present and valid,
/// else the shipped one.
pub fn active() -> Option<ListingModel> {
    load_user().or_else(builtin)
}

/// File in the data directory written by `ipo calibrate --save`.
pub const USER_FILE: &str = "ipo_model.json";

fn load_user() -> Option<ListingModel> {
    let m: Option<ListingModel> = crate::store::load::<Option<ListingModel>>(USER_FILE).ok()?;
    m.filter(|m| m.n >= 30 && m.residual_quantiles.len() == QUANTILES)
}

/// CDF of the residuals at `x`, linear between stored quantiles.
fn residual_cdf(q: &[f64], x: f64) -> f64 {
    if q.len() < 2 {
        return 0.5;
    }
    let step = 1.0 / (q.len() - 1) as f64;
    if x <= q[0] {
        return 0.0;
    }
    if x >= q[q.len() - 1] {
        return 1.0;
    }
    for i in 0..q.len() - 1 {
        let (a, b) = (q[i], q[i + 1]);
        if x >= a && x <= b {
            let t = if b > a { (x - a) / (b - a) } else { 1.0 };
            return (i as f64 + t) * step;
        }
    }
    1.0
}

fn quantile_of(q: &[f64], p: f64) -> f64 {
    if q.is_empty() {
        return 0.0;
    }
    let pos = p.clamp(0.0, 1.0) * (q.len() - 1) as f64;
    let i = pos.floor() as usize;
    if i + 1 >= q.len() {
        return q[q.len() - 1];
    }
    q[i] + (q[i + 1] - q[i]) * (pos - i as f64)
}

/// The `p`-quantile of `v` (linear interpolation, `v` sorted ascending).
fn sorted_quantile(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let pos = p.clamp(0.0, 1.0) * (v.len() - 1) as f64;
    let i = pos.floor() as usize;
    if i + 1 >= v.len() {
        return v[v.len() - 1];
    }
    v[i] + (v[i + 1] - v[i]) * (pos - i as f64)
}

// ── Regime ──

/// Recency-weighted median listing-day return (percent) of the listings in
/// `history` (listing date, return %) dated before `at` and within
/// [`REGIME_WINDOW_DAYS`]. `None` below [`REGIME_MIN_SAMPLE`] listings.
pub fn regime_asof(history: &[(NaiveDate, f64)], at: NaiveDate) -> Option<f64> {
    let mut pts: Vec<(f64, f64)> = history
        .iter()
        .filter(|(d, r)| *d < at && (at - *d).num_days() <= REGIME_WINDOW_DAYS && r.is_finite())
        .map(|(d, r)| {
            let age = (at - *d).num_days() as f64;
            (*r, 0.5f64.powf(age / REGIME_HALF_LIFE_DAYS))
        })
        .collect();
    if pts.len() < REGIME_MIN_SAMPLE {
        return None;
    }
    weighted_median(&mut pts)
}

/// Weighted median of (value, weight) pairs.
pub fn weighted_median(pts: &mut [(f64, f64)]) -> Option<f64> {
    let total: f64 = pts.iter().map(|p| p.1).sum();
    if pts.is_empty() || total <= 0.0 {
        return None;
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut acc = 0.0;
    for (i, (v, w)) in pts.iter().enumerate() {
        acc += w;
        if acc >= total / 2.0 {
            // Exactly half: average with the next value, like a plain median.
            if (acc - total / 2.0).abs() < 1e-12
                && let Some((next, _)) = pts.get(i + 1)
            {
                return Some((v + next) / 2.0);
            }
            return Some(*v);
        }
    }
    pts.last().map(|p| p.0)
}

// ── Fitting ──

/// One past mainboard issue for the fit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub symbol: String,
    pub listing_date: NaiveDate,
    pub issue_price: f64,
    pub qib_x: f64,
    pub nii_x: f64,
    pub retail_x: f64,
    pub listing_open: f64,
    pub listing_close: f64,
}

impl Sample {
    /// Listing-day close over the issue price, percent.
    pub fn listing_return_pct(&self) -> f64 {
        (self.listing_close / self.issue_price - 1.0) * 100.0
    }

    pub fn open_return_pct(&self) -> f64 {
        (self.listing_open / self.issue_price - 1.0) * 100.0
    }

    fn valid(&self) -> bool {
        self.issue_price > 0.0
            && self.listing_close > 0.0
            && self.listing_open > 0.0
            && [self.qib_x, self.nii_x, self.retail_x]
                .iter()
                .all(|x| x.is_finite() && *x >= 0.0)
    }
}

/// Why a fit could not be made.
#[derive(Debug, PartialEq)]
pub enum FitError {
    TooFew(usize),
    Singular,
}

impl std::fmt::Display for FitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FitError::TooFew(n) => write!(f, "only {n} usable issues; the fit needs at least 30"),
            FitError::Singular => {
                write!(
                    f,
                    "the inputs are collinear; the fit has no unique solution"
                )
            }
        }
    }
}

impl std::error::Error for FitError {}

/// Fit the model on `samples` (mainboard only). Each issue's regime is
/// computed from the issues listed before it in the same set. `huber`
/// down-weights outliers (iteratively reweighted least squares).
pub fn fit(samples: &[Sample], huber: bool, fit_date: NaiveDate) -> Result<ListingModel, FitError> {
    let mut rows: Vec<&Sample> = samples.iter().filter(|s| s.valid()).collect();
    rows.sort_by_key(|s| s.listing_date);
    if rows.len() < 30 {
        return Err(FitError::TooFew(rows.len()));
    }
    let history: Vec<(NaiveDate, f64)> = rows
        .iter()
        .map(|s| (s.listing_date, s.listing_return_pct()))
        .collect();
    let regimes: Vec<Option<f64>> = rows
        .iter()
        .map(|s| regime_asof(&history, s.listing_date))
        .collect();
    let known: Vec<f64> = regimes.iter().flatten().copied().collect();
    let regime_mean = if known.is_empty() {
        0.0
    } else {
        known.iter().sum::<f64>() / known.len() as f64
    };
    let x: Vec<[f64; 5]> = rows
        .iter()
        .zip(&regimes)
        .map(|(s, r)| {
            [
                1.0,
                (1.0 + s.qib_x).ln(),
                (1.0 + s.nii_x).ln(),
                (1.0 + s.retail_x).ln(),
                r.unwrap_or(regime_mean),
            ]
        })
        .collect();
    let y: Vec<f64> = rows.iter().map(|s| s.listing_return_pct()).collect();
    let mut w = vec![1.0; y.len()];
    let mut beta = wls(&x, &y, &w).ok_or(FitError::Singular)?;
    if huber {
        for _ in 0..50 {
            let res: Vec<f64> = x
                .iter()
                .zip(&y)
                .map(|(xi, yi)| yi - dot(xi, &beta))
                .collect();
            let scale = mad(&res).max(1e-6) / 0.6745;
            w = res
                .iter()
                .map(|r| {
                    let u = (r / scale).abs();
                    if u <= HUBER_K { 1.0 } else { HUBER_K / u }
                })
                .collect();
            let next = wls(&x, &y, &w).ok_or(FitError::Singular)?;
            let moved = next
                .iter()
                .zip(&beta)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f64::max);
            beta = next;
            if moved < 1e-9 {
                break;
            }
        }
    }
    let res: Vec<f64> = x
        .iter()
        .zip(&y)
        .map(|(xi, yi)| yi - dot(xi, &beta))
        .collect();
    let n = y.len();
    let mean_y = y.iter().sum::<f64>() / n as f64;
    let sst: f64 = y.iter().map(|v| (v - mean_y).powi(2)).sum();
    let ssr: f64 = res.iter().map(|r| r * r).sum();
    let r2 = if sst > 0.0 { 1.0 - ssr / sst } else { 0.0 };
    let resid_sd = (ssr / (n - 5) as f64).sqrt();
    let mut sorted = res.clone();
    sorted.sort_by(f64::total_cmp);
    let residual_quantiles = (0..QUANTILES)
        .map(|i| round2(sorted_quantile(&sorted, i as f64 / (QUANTILES - 1) as f64)))
        .collect();
    Ok(ListingModel {
        intercept: round4(beta[0]),
        b_qib: round4(beta[1]),
        b_nii: round4(beta[2]),
        b_retail: round4(beta[3]),
        b_regime: round4(beta[4]),
        regime_mean: round4(regime_mean),
        fit_date: fit_date.to_string(),
        n,
        sample_from: rows[0].listing_date.to_string(),
        sample_to: rows[n - 1].listing_date.to_string(),
        r2: round4(r2),
        resid_sd: round4(resid_sd),
        residual_quantiles,
        method: if huber { "huber" } else { "ols" }.to_string(),
    })
}

/// The fitted model as the `BUILTIN` constant, for pasting into this file.
pub fn as_const(m: &ListingModel) -> String {
    let q: Vec<String> = m
        .residual_quantiles
        .iter()
        .map(|v| format!("{v:.2}"))
        .collect();
    format!(
        "const BUILTIN: Option<Builtin> = Some((\n    [{:.4}, {:.4}, {:.4}, {:.4}, {:.4}],\n    {:.4},\n    \"{}\",\n    {},\n    \"{}\",\n    \"{}\",\n    {:.4},\n    {:.4},\n    [{}],\n));",
        m.intercept,
        m.b_qib,
        m.b_nii,
        m.b_retail,
        m.b_regime,
        m.regime_mean,
        m.fit_date,
        m.n,
        m.sample_from,
        m.sample_to,
        m.r2,
        m.resid_sd,
        q.join(", ")
    )
}

fn dot(a: &[f64; 5], b: &[f64; 5]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn mad(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let med = sorted_quantile(&s, 0.5);
    let mut d: Vec<f64> = v.iter().map(|x| (x - med).abs()).collect();
    d.sort_by(f64::total_cmp);
    sorted_quantile(&d, 0.5)
}

/// Weighted least squares through the normal equations (5 unknowns),
/// solved by Gauss-Jordan elimination with partial pivoting.
fn wls(x: &[[f64; 5]], y: &[f64], w: &[f64]) -> Option<[f64; 5]> {
    let mut a = [[0.0f64; 6]; 5];
    for ((xi, yi), wi) in x.iter().zip(y).zip(w) {
        for r in 0..5 {
            for c in 0..5 {
                a[r][c] += wi * xi[r] * xi[c];
            }
            a[r][5] += wi * xi[r] * yi;
        }
    }
    for col in 0..5 {
        let pivot = (col..5).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-10 {
            return None;
        }
        a.swap(col, pivot);
        let p = a[col][col];
        for v in a[col].iter_mut() {
            *v /= p;
        }
        for r in 0..5 {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    let pivot_row = a[col];
                    for (v, pv) in a[r].iter_mut().zip(pivot_row) {
                        *v -= f * pv;
                    }
                }
            }
        }
    }
    Some([a[0][5], a[1][5], a[2][5], a[3][5], a[4][5]])
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Read calibration rows from CSV with a header naming at least `symbol`,
/// `listing_date` (YYYY-MM-DD), `issue_price`, `qib_x`, `nii_x` and
/// `retail_x`; `listing_open` and `listing_close` are optional (0 when
/// absent or empty, for the caller to fill from daily bars). Rows that do
/// not parse are reported by line number.
pub fn samples_from_csv(text: &str) -> (Vec<Sample>, Vec<String>) {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty());
    let Some((_, header)) = lines.next() else {
        return (Vec::new(), vec!["empty file".to_string()]);
    };
    let cols: Vec<String> = header
        .split(',')
        .map(|c| c.trim().trim_matches('"').to_ascii_lowercase())
        .collect();
    let idx = |name: &str| cols.iter().position(|c| c == name);
    let need = [
        "symbol",
        "listing_date",
        "issue_price",
        "qib_x",
        "nii_x",
        "retail_x",
    ];
    let pos: Vec<Option<usize>> = need.iter().map(|n| idx(n)).collect();
    if let Some(missing) = need.iter().zip(&pos).find(|(_, p)| p.is_none()) {
        return (Vec::new(), vec![format!("missing column {}", missing.0)]);
    }
    let pos: Vec<usize> = pos.into_iter().flatten().collect();
    let (open_at, close_at) = (idx("listing_open"), idx("listing_close"));
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (n, line) in lines {
        let f: Vec<&str> = line
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .collect();
        let num = |i: usize| f.get(pos[i]).and_then(|s| s.parse::<f64>().ok());
        let opt = |at: Option<usize>| {
            at.and_then(|i| f.get(i))
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let row = (|| {
            Some(Sample {
                symbol: f.get(pos[0]).filter(|s| !s.is_empty())?.to_string(),
                listing_date: NaiveDate::parse_from_str(f.get(pos[1])?, "%Y-%m-%d").ok()?,
                issue_price: num(2).filter(|p| *p > 0.0)?,
                qib_x: num(3).filter(|x| *x >= 0.0)?,
                nii_x: num(4).filter(|x| *x >= 0.0)?,
                retail_x: num(5).filter(|x| *x >= 0.0)?,
                listing_open: opt(open_at),
                listing_close: opt(close_at),
            })
        })();
        match row {
            Some(s) => out.push(s),
            None => errors.push(format!("line {}: not a usable row", n + 1)),
        }
    }
    (out, errors)
}

/// The rows as CSV, for `ipo_calibration.csv`.
pub fn samples_to_csv(rows: &[Sample]) -> String {
    let mut o = String::from(
        "symbol,listing_date,issue_price,qib_x,nii_x,retail_x,listing_open,listing_close\n",
    );
    for s in rows {
        o.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            s.symbol,
            s.listing_date,
            s.issue_price,
            s.qib_x,
            s.nii_x,
            s.retail_x,
            s.listing_open,
            s.listing_close
        ));
    }
    o
}

/// In-sample check of P(loss): for each band of predicted probability, the
/// issues in it and the share that actually closed listing day below issue.
pub fn loss_calibration(m: &ListingModel, rows: &[Sample]) -> Vec<(String, usize, Option<f64>)> {
    let history: Vec<(NaiveDate, f64)> = rows
        .iter()
        .map(|s| (s.listing_date, s.listing_return_pct()))
        .collect();
    let bands = [(0.0, 0.1), (0.1, 0.25), (0.25, 0.5), (0.5, 1.01)];
    bands
        .iter()
        .map(|(lo, hi)| {
            let hits: Vec<bool> = rows
                .iter()
                .filter(|s| s.valid())
                .filter_map(|s| {
                    let f = Features {
                        qib_x: s.qib_x,
                        nii_x: s.nii_x,
                        retail_x: s.retail_x,
                        regime_pct: regime_asof(&history, s.listing_date),
                    };
                    let p = m.p_loss(m.predict(&f));
                    (p >= *lo && p < *hi).then_some(s.listing_return_pct() < 0.0)
                })
                .collect();
            let share = (!hits.is_empty())
                .then(|| hits.iter().filter(|h| **h).count() as f64 / hits.len() as f64);
            (
                format!("{:.0}–{:.0}%", lo * 100.0, hi.min(1.0) * 100.0),
                hits.len(),
                share,
            )
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A fixed model for verdict tests (not the shipped fit): +2% at no
    /// demand, rising with each category, regime weight 0.3.
    pub fn model() -> ListingModel {
        // Residuals roughly normal with sd 15.
        let z = [
            -2.6, -1.645, -1.28, -1.04, -0.84, -0.67, -0.52, -0.385, -0.25, -0.125, 0.0, 0.125,
            0.25, 0.385, 0.52, 0.67, 0.84, 1.04, 1.28, 1.645, 2.6,
        ];
        ListingModel {
            intercept: -6.0,
            b_qib: 6.0,
            b_nii: 3.0,
            b_retail: 2.0,
            b_regime: 0.3,
            regime_mean: 10.0,
            fit_date: "2026-11-21".into(),
            n: 200,
            sample_from: "2023-01-01".into(),
            sample_to: "2026-10-31".into(),
            r2: 0.45,
            resid_sd: 15.0,
            residual_quantiles: z.iter().map(|v| v * 15.0).collect(),
            method: "huber".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// 60 synthetic issues whose returns follow a known formula exactly.
    fn synthetic(noise: impl Fn(usize) -> f64) -> Vec<Sample> {
        (0..60)
            .map(|i| {
                let q = (i % 10) as f64 * 7.0 + 0.5;
                let n = ((i * 7) % 13) as f64 * 3.0 + 0.2;
                let r = ((i * 3) % 11) as f64 * 1.5 + 0.1;
                let ret = -5.0
                    + 8.0 * (1.0 + q).ln()
                    + 2.0 * (1.0 + n).ln()
                    + 1.0 * (1.0 + r).ln()
                    + noise(i);
                Sample {
                    symbol: format!("S{i}"),
                    // One listing a week: the first 8 have no regime.
                    listing_date: d("2024-01-01") + chrono::Duration::days(7 * i as i64),
                    issue_price: 100.0,
                    qib_x: q,
                    nii_x: n,
                    retail_x: r,
                    listing_open: 100.0 + ret,
                    listing_close: 100.0 + ret,
                }
            })
            .collect()
    }

    #[test]
    fn ols_recovers_known_coefficients() {
        let s = synthetic(|_| 0.0);
        let m = fit(&s, false, d("2026-11-21")).unwrap();
        // The regime here is a function of the other inputs' history; with
        // no noise the fit is exact either way.
        let pred = m.predict(&Features {
            qib_x: 20.0,
            nii_x: 5.0,
            retail_x: 3.0,
            regime_pct: None,
        });
        let truth = -5.0 + 8.0 * 21f64.ln() + 2.0 * 6f64.ln() + 1.0 * 4f64.ln();
        assert!((pred - truth).abs() < 0.5, "{pred} vs {truth}");
        assert!(m.r2 > 0.999, "{}", m.r2);
        assert_eq!(m.n, 60);
        assert_eq!(m.residual_quantiles.len(), QUANTILES);
    }

    #[test]
    fn huber_resists_an_outlier() {
        let clean = synthetic(|i| if i % 2 == 0 { 0.5 } else { -0.5 });
        let dirty = synthetic(|i| match i {
            5 => 400.0,
            i if i % 2 == 0 => 0.5,
            _ => -0.5,
        });
        let base = fit(&clean, true, d("2026-11-21")).unwrap();
        let ols = fit(&dirty, false, d("2026-11-21")).unwrap();
        let rob = fit(&dirty, true, d("2026-11-21")).unwrap();
        assert!(
            (rob.b_qib - base.b_qib).abs() < (ols.b_qib - base.b_qib).abs(),
            "huber {} ols {} clean {}",
            rob.b_qib,
            ols.b_qib,
            base.b_qib
        );
        assert_eq!(rob.method, "huber");
    }

    #[test]
    fn too_few_issues_is_an_error() {
        let s: Vec<Sample> = synthetic(|_| 0.0).into_iter().take(10).collect();
        assert_eq!(fit(&s, true, d("2026-11-21")), Err(FitError::TooFew(10)));
    }

    #[test]
    fn p_loss_reads_the_residual_distribution() {
        let m = testing::model();
        // Expected 0%: half the residuals are below zero.
        assert!((m.p_loss(0.0) - 0.5).abs() < 1e-9);
        // Expected +15% (one sd): about 16% chance of a loss.
        let p = m.p_loss(15.0);
        assert!((0.14..0.18).contains(&p), "{p}");
        // Far beyond the worst residual: no loss in the sample.
        assert_eq!(m.p_loss(100.0), 0.0);
        assert_eq!(m.p_loss(-100.0), 1.0);
        assert!((m.quantile(10.0, 0.5) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn qib_threshold_is_where_the_condition_starts_to_hold() {
        let m = testing::model();
        let f = Features {
            qib_x: 0.0,
            nii_x: 2.0,
            retail_x: 2.0,
            regime_pct: Some(10.0),
        };
        let q = m.qib_threshold(&f, |mu, p| mu > 0.0 && p < 0.25).unwrap();
        let at = m.predict(&Features { qib_x: q, ..f });
        assert!(m.p_loss(at) < 0.25 + 1e-6);
        let below = m.predict(&Features {
            qib_x: (q * 0.95 - 0.01).max(0.0),
            ..f
        });
        assert!(m.p_loss(below) >= 0.25 - 1e-3);
        // A condition no QIB level meets.
        assert_eq!(m.qib_threshold(&f, |mu, _| mu > 500.0), None);
    }

    #[test]
    fn regime_is_recency_weighted_median_with_min_sample() {
        let at = d("2026-11-20");
        let seven: Vec<(NaiveDate, f64)> = (0..7)
            .map(|i| (at - chrono::Duration::days(10 + i), 20.0))
            .collect();
        assert_eq!(regime_asof(&seven, at), None, "7 listings is too few");
        // Four recent losers (-5%) outweigh five old winners (+40%).
        let mut h: Vec<(NaiveDate, f64)> = (0..4)
            .map(|i| (at - chrono::Duration::days(3 + i), -5.0))
            .collect();
        h.extend((0..5).map(|i| (at - chrono::Duration::days(150 + i), 40.0)));
        assert_eq!(regime_asof(&h, at), Some(-5.0));
        // A plain median would say +40%.
        let mut plain: Vec<(f64, f64)> = h.iter().map(|(_, r)| (*r, 1.0)).collect();
        assert_eq!(weighted_median(&mut plain), Some(40.0));
        // Listings older than the window or on/after the date are ignored.
        let mut old = h.clone();
        old.push((at - chrono::Duration::days(400), -90.0));
        old.push((at, -90.0));
        assert_eq!(regime_asof(&old, at), Some(-5.0));
    }

    #[test]
    fn csv_rows_parse_and_bad_rows_are_reported() {
        let text = "symbol,listing_date,issue_price,qib_x,nii_x,retail_x,listing_open,listing_close\n\
                    AAA,2025-01-10,100,50,20,5,130,125\n\
                    BBB,2025-13-10,100,1,1,1,100,100\n\
                    CCC,2025-02-10,100,1,1,1,,\n";
        let (rows, errors) = samples_from_csv(text);
        assert_eq!(rows.len(), 2);
        // Prices left for the caller to fill.
        assert_eq!(rows[1].listing_close, 0.0);
        assert!(!rows[1].valid());
        let back = samples_from_csv(&samples_to_csv(&rows[..1])).0;
        assert_eq!(back, rows[..1]);
        assert_eq!(rows[0].listing_return_pct(), 25.0);
        assert!((rows[0].open_return_pct() - 30.0).abs() < 1e-9);
        assert_eq!(errors, ["line 3: not a usable row"]);
        let (_, e) = samples_from_csv("symbol,issue_price\n");
        assert_eq!(e, ["missing column listing_date"]);
    }

    #[test]
    fn const_round_trips_the_shape() {
        let c = as_const(&testing::model());
        assert!(c.starts_with("const BUILTIN: Option<Builtin> = Some(("));
        assert_eq!(c.matches(", ").count() >= 24, true);
    }
}
