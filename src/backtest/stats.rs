//! How much a backtest's Sharpe ratio can be trusted.
//!
//! - Probabilistic Sharpe Ratio (PSR): the probability that the true Sharpe
//!   ratio is above a benchmark, given how many returns were observed and
//!   how skewed and fat-tailed they were.
//! - Deflated Sharpe Ratio (DSR): the PSR against the best Sharpe ratio that
//!   luck alone would produce after trying `N` parameter sets. Trying many
//!   variants and keeping the best one inflates the Sharpe ratio; the DSR
//!   takes that selection back out.
//!
//! Both describe the historical sample only. They are not a forecast.
//!
//! References:
//! - D. H. Bailey and M. López de Prado, "The Sharpe Ratio Efficient
//!   Frontier", Journal of Risk 15(2), 2012 (PSR, eq. 6 there).
//! - D. H. Bailey and M. López de Prado, "The Deflated Sharpe Ratio:
//!   Correcting for Selection Bias, Backtest Overfitting and Non-Normality",
//!   Journal of Portfolio Management 40(5), 2014 (DSR, eqs. 1-2 there).

use super::Bars;
use super::engine::{self, Asset, SimConfig, SimResult};
use super::rules::StrategyDef;
use anyhow::Result;
use serde::Serialize;

/// Euler-Mascheroni constant γ.
const EULER_GAMMA: f64 = std::f64::consts::EULER_GAMMA;

/// Standard normal CDF Φ(x), accurate to about 1e-14: Hart's (1968)
/// rational approximation as given by G. West, "Better approximations to
/// cumulative normal functions", Wilmott Magazine, 2005.
pub fn norm_cdf(x: f64) -> f64 {
    let z = x.abs();
    let tail = if z > 37.0 {
        0.0
    } else {
        let e = (-z * z / 2.0).exp();
        if z < 7.071_067_811_865_47 {
            let n = (((((0.035_262_496_599_891_1 * z + 0.700_383_064_443_688) * z
                + 6.373_962_203_531_65)
                * z
                + 33.912_866_078_383)
                * z
                + 112.079_291_497_871)
                * z
                + 221.213_596_169_931)
                * z
                + 220.206_867_912_376;
            let d = ((((((0.088_388_347_648_318_4 * z + 1.755_667_163_182_64) * z
                + 16.064_177_579_207)
                * z
                + 86.780_732_202_946_1)
                * z
                + 296.564_248_779_674)
                * z
                + 637.333_633_378_831)
                * z
                + 793.826_512_519_948)
                * z
                + 440.413_735_824_752;
            e * n / d
        } else {
            let b = z + 1.0 / (z + 2.0 / (z + 3.0 / (z + 4.0 / (z + 0.65))));
            e / b / 2.506_628_274_631
        }
    };
    if x > 0.0 { 1.0 - tail } else { tail }
}

/// Inverse standard normal CDF Φ⁻¹(p) for 0 < p < 1: Acklam's rational
/// approximation (relative error below 1.2e-9), refined by one Halley step.
pub fn norm_ppf(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    let x = if p < 0.024_25 {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - 0.024_25 {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    };
    // Halley refinement against the CDF above.
    let e = norm_cdf(x) - p;
    let u = e * (2.0 * std::f64::consts::PI).sqrt() * (x * x / 2.0).exp();
    x - u / (1.0 + x * u / 2.0)
}

/// Per-period Sharpe ratio and the shape of the returns behind it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moments {
    /// Mean ÷ sample standard deviation, not annualised.
    pub sharpe: f64,
    /// Number of returns.
    pub n: usize,
    pub skewness: f64,
    /// Raw kurtosis (3 for a normal distribution, not excess kurtosis).
    pub kurtosis: f64,
}

/// Moments of the period-to-period returns of an equity curve. `None` with
/// fewer than three returns or no variation.
pub fn moments(curve: &[f64]) -> Option<Moments> {
    let r: Vec<f64> = curve
        .windows(2)
        .filter(|w| w[0] != 0.0)
        .map(|w| w[1] / w[0] - 1.0)
        .collect();
    let n = r.len();
    if n < 3 {
        return None;
    }
    let nf = n as f64;
    let mean = r.iter().sum::<f64>() / nf;
    let central = |k: i32| r.iter().map(|x| (x - mean).powi(k)).sum::<f64>() / nf;
    let (m2, m3, m4) = (central(2), central(3), central(4));
    if m2 <= 0.0 {
        return None;
    }
    let sd = (m2 * nf / (nf - 1.0)).sqrt();
    Some(Moments {
        sharpe: mean / sd,
        n,
        skewness: m3 / m2.powf(1.5),
        kurtosis: m4 / (m2 * m2),
    })
}

/// Probabilistic Sharpe Ratio: P(true Sharpe > `benchmark`), both per period.
///
/// PSR(SR*) = Φ( (SR − SR*) · √(T − 1) / √(1 − γ₃·SR + (γ₄ − 1)/4 · SR²) )
///
/// with T returns, skewness γ₃ and raw kurtosis γ₄ (Bailey & López de
/// Prado 2012, eq. 6). `None` when the denominator is not positive.
pub fn probabilistic_sharpe(m: &Moments, benchmark: f64) -> Option<f64> {
    let sr = m.sharpe;
    let var = 1.0 - m.skewness * sr + (m.kurtosis - 1.0) / 4.0 * sr * sr;
    (var > 0.0 && m.n > 1)
        .then(|| norm_cdf((sr - benchmark) * ((m.n - 1) as f64).sqrt() / var.sqrt()))
}

/// The Sharpe ratio expected from the best of `trials` unskilled strategies
/// whose Sharpe ratios vary with `variance` (per period):
///
/// SR₀ = √V · ( (1 − γ)·Φ⁻¹(1 − 1/N) + γ·Φ⁻¹(1 − 1/(N·e)) )
///
/// (Bailey & López de Prado 2014, eq. 2; γ is the Euler-Mascheroni
/// constant). Zero for a single trial.
pub fn expected_max_sharpe(trials: usize, variance: f64) -> f64 {
    if trials < 2 || variance <= 0.0 {
        return 0.0;
    }
    let n = trials as f64;
    variance.sqrt()
        * ((1.0 - EULER_GAMMA) * norm_ppf(1.0 - 1.0 / n)
            + EULER_GAMMA * norm_ppf(1.0 - 1.0 / (n * std::f64::consts::E)))
}

/// Deflated Sharpe Ratio: the PSR against [`expected_max_sharpe`].
pub fn deflated_sharpe(m: &Moments, trials: usize, variance: f64) -> Option<f64> {
    probabilistic_sharpe(m, expected_max_sharpe(trials, variance))
}

/// PSR and DSR for a backtest, as reported.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SharpeTest {
    /// P(true Sharpe > 0) from this sample.
    pub probabilistic_sharpe: f64,
    /// P(true Sharpe > what the best of `trials` random variants would show).
    pub deflated_sharpe: f64,
    /// Parameter sets tried: the `[grid]` combinations, plus the reported
    /// parameters when they are not one of them (1 without a grid).
    pub trials: usize,
    /// Annualised Sharpe that luck alone would give the best trial.
    pub expected_max_sharpe: f64,
    pub skewness: f64,
    pub kurtosis: f64,
    pub observations: usize,
}

/// PSR and DSR for `curve`, where `trial_sharpes` holds one per-period
/// Sharpe ratio for every parameter set tried, the reported one included
/// (see [`trial_sharpe`]). With fewer than two trials, or no spread between
/// them, nothing is deflated and the DSR equals the PSR.
pub fn sharpe_test(
    curve: &[f64],
    trial_sharpes: &[f64],
    periods_per_year: f64,
) -> Option<SharpeTest> {
    let m = moments(curve)?;
    let trials = trial_sharpes.len().max(1);
    let variance = sample_variance(trial_sharpes).unwrap_or(0.0);
    let sr0 = expected_max_sharpe(trials, variance);
    Some(SharpeTest {
        probabilistic_sharpe: probabilistic_sharpe(&m, 0.0)?,
        deflated_sharpe: probabilistic_sharpe(&m, sr0)?,
        trials,
        expected_max_sharpe: sr0 * periods_per_year.sqrt(),
        skewness: m.skewness,
        kurtosis: m.kurtosis,
        observations: m.n,
    })
}

/// The equity curve with the starting capital in front.
pub fn curve(r: &SimResult) -> Vec<f64> {
    let mut c = Vec::with_capacity(r.equity.len() + 1);
    c.push(r.capital);
    c.extend_from_slice(&r.equity);
    c
}

/// One trial's per-period Sharpe ratio. A curve with no variation (a
/// parameter set that never traded) earned nothing over cash: 0. It still
/// counts as a trial.
pub fn trial_sharpe(curve: &[f64]) -> f64 {
    moments(curve).map_or(0.0, |m| m.sharpe)
}

/// Per-period Sharpe ratio of every parameter set in the strategy's grid,
/// plus its current parameters when the grid does not contain them, each
/// run over the same bars with the same settings. These are the trials the
/// Deflated Sharpe Ratio corrects for; there is one per parameter set.
pub fn grid_trials(def: &StrategyDef, data: &[(&str, &Bars)], cfg: &SimConfig) -> Result<Vec<f64>> {
    let mut combos = def.grid_combos();
    if !combos.contains(&def.params) {
        combos.push(def.params.clone());
    }
    let mut out = Vec::with_capacity(combos.len());
    for combo in combos {
        let s = def.compile_with(&combo)?;
        let signals = data
            .iter()
            .map(|(_, b)| s.signals(b))
            .collect::<Result<Vec<_>>>()?;
        let assets: Vec<Asset> = data
            .iter()
            .zip(&signals)
            .map(|((symbol, bars), signals)| Asset {
                symbol,
                bars,
                signals,
            })
            .collect();
        let c = SimConfig {
            stop_loss_pct: s.stop_loss_pct,
            take_profit_pct: s.take_profit_pct,
            execution: s.execution.clone(),
            ..cfg.clone()
        };
        out.push(trial_sharpe(&curve(&engine::simulate(&assets, &c))));
    }
    Ok(out)
}

fn sample_variance(v: &[f64]) -> Option<f64> {
    if v.len() < 2 {
        return None;
    }
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    Some(v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() < tol, "got {a}, want {b}");
    }

    #[test]
    fn normal_distribution_reference_values() {
        close(norm_cdf(0.0), 0.5, 1e-12);
        close(norm_cdf(1.959_963_984_540_054), 0.975, 1e-12);
        close(norm_cdf(-1.0), 0.158_655_253_931_457_05, 1e-12);
        close(norm_ppf(0.975), 1.959_963_984_540_054, 1e-9);
        close(norm_ppf(0.99), 2.326_347_874_040_841, 1e-9);
        close(norm_ppf(0.001), -3.090_232_306_167_813_5, 1e-9);
    }

    /// The worked example in Bailey & López de Prado (2014), section 3:
    /// 100 trials whose annualised Sharpe ratios vary by 0.5, a best
    /// annualised Sharpe of 2.5 over 1,250 daily returns (250 a year) with
    /// skewness −3 and kurtosis 10. The paper gets SR₀ ≈ 0.1132 per day
    /// and DSR ≈ 0.9004.
    #[test]
    fn deflated_sharpe_matches_the_paper_example() {
        let variance = 0.5 / 250.0;
        let sr0 = expected_max_sharpe(100, variance);
        close(sr0, 0.1132, 1e-4);
        let m = Moments {
            sharpe: 2.5 / 250f64.sqrt(),
            n: 1_250,
            skewness: -3.0,
            kurtosis: 10.0,
        };
        close(deflated_sharpe(&m, 100, variance).unwrap(), 0.9004, 5e-4);
    }

    /// With normal returns (skewness 0, kurtosis 3) the PSR reduces to
    /// Φ(SR · √(T−1) / √(1 + SR²/2)). SR = 0.1 over 101 returns:
    /// 0.1 · 10 / √1.005 = 0.997509 → Φ = 0.840741.
    #[test]
    fn probabilistic_sharpe_for_normal_returns() {
        let m = Moments {
            sharpe: 0.1,
            n: 101,
            skewness: 0.0,
            kurtosis: 3.0,
        };
        close(probabilistic_sharpe(&m, 0.0).unwrap(), 0.840_741, 1e-5);
        // Against its own Sharpe ratio the probability is one half.
        close(probabilistic_sharpe(&m, 0.1).unwrap(), 0.5, 1e-9);
        // One trial: nothing to deflate.
        assert_eq!(deflated_sharpe(&m, 1, 0.01), probabilistic_sharpe(&m, 0.0));
    }

    #[test]
    fn negative_skew_and_fat_tails_lower_the_psr() {
        let normal = Moments {
            sharpe: 0.1,
            n: 252,
            skewness: 0.0,
            kurtosis: 3.0,
        };
        let crashy = Moments {
            skewness: -2.0,
            kurtosis: 12.0,
            ..normal
        };
        assert!(
            probabilistic_sharpe(&crashy, 0.0).unwrap()
                < probabilistic_sharpe(&normal, 0.0).unwrap()
        );
    }

    #[test]
    fn moments_of_a_curve() {
        // Returns +10%, −10%, +10%, −10%: mean 0, symmetric.
        let m = moments(&[100.0, 110.0, 99.0, 108.9, 98.01]).unwrap();
        close(m.sharpe, 0.0, 1e-12);
        close(m.skewness, 0.0, 1e-9);
        close(m.kurtosis, 1.0, 1e-9); // two-point distribution
        assert_eq!(m.n, 4);
        assert!(moments(&[100.0, 100.0, 100.0, 100.0]).is_none());
        assert!(moments(&[100.0, 101.0]).is_none());
    }

    #[test]
    fn every_attempted_combination_is_a_trial() {
        let def = crate::backtest::rules::StrategyDef::parse(
            "entry = \"close > $level\"\nexit = \"close < 50\"\n\
             [params]\nlevel = 5\n[grid]\nlevel = [10, 1000]",
            "t",
            "t.toml",
        )
        .unwrap();
        let closes: Vec<f64> = (0..60)
            .map(|i| 100.0 + (i as f64 * 0.5).sin() * 5.0)
            .collect();
        let bars = Bars::from_closes(&closes);
        let cfg = SimConfig::single(
            10_000.0,
            engine::Fill::NextOpen,
            crate::backtest::costs::CostModel::none(),
        );
        // level = 1000 never trades but counts (Sharpe 0), and the default
        // level = 5 is outside the grid, so it is added: three trials.
        let t = grid_trials(&def, &[("X", &bars)], &cfg).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[1], 0.0);
        assert!(t[0] != 0.0 && t[2] != 0.0);
        // Defaults inside the grid are not counted twice.
        let pinned = def.with_params(&[("level".into(), 10.0)]).unwrap();
        assert_eq!(
            grid_trials(&pinned, &[("X", &bars)], &cfg).unwrap().len(),
            1
        );
    }

    #[test]
    fn more_trials_deflate_more() {
        let curve: Vec<f64> = (0..300)
            .map(|i| 100.0 * (1.0 + 0.001 * i as f64 + 0.01 * (i as f64).sin()))
            .collect();
        let one = sharpe_test(&curve, &[0.05], 252.0).unwrap();
        assert_eq!(one.trials, 1);
        assert_eq!(one.deflated_sharpe, one.probabilistic_sharpe);
        let many = sharpe_test(&curve, &[0.05, 0.02, -0.01, 0.08, 0.0, 0.03], 252.0).unwrap();
        assert_eq!(many.trials, 6);
        assert!(many.deflated_sharpe < many.probabilistic_sharpe);
        assert!(many.expected_max_sharpe > 0.0);
    }
}
