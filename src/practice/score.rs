//! The 0–100 score, grade and badges for a finished run.
//!
//! Raw return never scores on its own. The score adds four parts:
//!
//! | Part             | Points | What earns them                                              |
//! |------------------|-------:|--------------------------------------------------------------|
//! | Risk-adjusted    | 40     | Sharpe and Sortino ratios compared with the benchmark's       |
//! | Drawdown control | 20     | a worst drawdown small next to the benchmark's, scaled to your exposure (10% floor) |
//! | Discipline       | 25     | buys with a stop-loss, positions at most 20%, rules kept      |
//! | Diversification  | 15     | sectors held at once (5 for full marks) and stocks used (8)   |
//!
//! The first two scale with how invested you were (full credit from 30% on
//! average): sitting in cash through a crash avoids the loss but is not a
//! trading decision worth points. A run under five trading days is not
//! scored. Everything here is a pure function of the run.

use crate::backtest::engine::{sharpe, sortino};
use crate::i18n::Msg;
use crate::output::round;
use serde::{Deserialize, Serialize};

/// Trading days a run needs before it is scored.
pub const MIN_DAYS: usize = 5;
/// The position size (percent of equity) the discipline part allows.
pub const POSITION_LIMIT: f64 = 20.0;
/// Average invested share for full risk-adjusted and drawdown credit.
const FULL_PARTICIPATION: f64 = 30.0;

pub const DISCLAIMER: Msg = Msg::PracticeDisclaimer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuyFact {
    pub with_stop: bool,
    /// The position's share of equity after the buy.
    pub weight_percent: f64,
}

/// Everything the score is computed from.
#[derive(Debug, Clone, PartialEq)]
pub struct RunInput {
    /// Time-weighted value of the account, starting at 1 (see [`nav`]).
    pub nav: Vec<f64>,
    /// Benchmark closes on the same days.
    pub benchmark: Vec<f64>,
    /// Invested share of equity at each close after the first, 0–1.
    pub exposure: Vec<f64>,
    pub buys: Vec<BuyFact>,
    /// Most sectors held at one close.
    pub max_sectors: usize,
    /// Distinct stocks held during the run.
    pub symbols_held: usize,
    pub hard_break: bool,
    pub soft_violations: usize,
    pub periods_per_year: f64,
}

/// Points per part.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Parts {
    pub risk_adjusted: f64,
    pub drawdown: f64,
    pub discipline: f64,
    pub diversification: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scorecard {
    pub days: usize,
    /// 0–100, or None when the run is shorter than [`MIN_DAYS`].
    pub score: Option<u32>,
    pub grade: Option<String>,
    pub return_percent: f64,
    pub benchmark_return_percent: f64,
    pub max_drawdown_percent: f64,
    pub benchmark_max_drawdown_percent: f64,
    pub sharpe: Option<f64>,
    pub benchmark_sharpe: Option<f64>,
    pub sortino: Option<f64>,
    pub benchmark_sortino: Option<f64>,
    /// Average invested share of equity.
    pub exposure_percent: f64,
    pub parts: Parts,
}

/// Time-weighted account value from daily equity and the money added
/// before each day (`flows[i]` arrives before `equity[i]` is measured), so
/// contributions don't count as gains. Starts at 1.
pub fn nav(equity: &[f64], flows: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(equity.len());
    for (i, &e) in equity.iter().enumerate() {
        let v = match i {
            0 => 1.0,
            _ if equity[i - 1] > 0.0 => {
                let f = flows.get(i).copied().unwrap_or(0.0);
                out[i - 1] * (e - f) / equity[i - 1]
            }
            _ => out[i - 1],
        };
        out.push(v);
    }
    out
}

/// Worst peak-to-trough fall of `curve`, in percent.
pub fn max_drawdown_percent(curve: &[f64]) -> f64 {
    crate::technical::max_drawdown(curve)
        .map(|(dd, _, _)| dd * 100.0)
        .unwrap_or(0.0)
}

/// How far `curve` is below its own peak at its last point, in percent.
pub fn current_drawdown_percent(curve: &[f64]) -> f64 {
    let peak = curve.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    match curve.last() {
        Some(&last) if peak > 0.0 => (1.0 - last / peak) * 100.0,
        _ => 0.0,
    }
}

fn change_percent(curve: &[f64]) -> f64 {
    match (curve.first(), curve.last()) {
        (Some(&a), Some(&b)) if a > 0.0 => (b / a - 1.0) * 100.0,
        _ => 0.0,
    }
}

pub fn grade(score: u32) -> &'static str {
    match score {
        90.. => "A+",
        80..=89 => "A",
        70..=79 => "B",
        60..=69 => "C",
        50..=59 => "D",
        _ => "F",
    }
}

/// A ratio for comparison, capped at ±4. A curve that never fell has no
/// Sortino ratio: it counts as the cap when it rose, else as 0.
fn capped(ratio: Option<f64>, rose: bool) -> f64 {
    match ratio {
        Some(r) => r.clamp(-4.0, 4.0),
        None if rose => 4.0,
        None => 0.0,
    }
}

/// Half credit for matching the benchmark, full credit two ratio points
/// above it, none two below.
fn relative(you: f64, bench: f64) -> f64 {
    (0.5 + (you - bench) / 4.0).clamp(0.0, 1.0)
}

pub fn score(input: &RunInput) -> Scorecard {
    let ppy = input.periods_per_year;
    let days = input.nav.len().saturating_sub(1);
    let ret = change_percent(&input.nav);
    let bench_ret = change_percent(&input.benchmark);
    let dd = max_drawdown_percent(&input.nav);
    let bench_dd = max_drawdown_percent(&input.benchmark);
    let (s, bs) = (sharpe(&input.nav, ppy), sharpe(&input.benchmark, ppy));
    let (so, bso) = (sortino(&input.nav, ppy), sortino(&input.benchmark, ppy));
    let exposure = if input.exposure.is_empty() {
        0.0
    } else {
        input.exposure.iter().sum::<f64>() / input.exposure.len() as f64 * 100.0
    };

    let participation = (exposure / FULL_PARTICIPATION).min(1.0);
    let risk = 0.5 * relative(capped(s, ret > 0.0), capped(bs, bench_ret > 0.0))
        + 0.5 * relative(capped(so, ret > 0.0), capped(bso, bench_ret > 0.0));
    // Judge the drawdown against what the benchmark would have lost at the
    // same exposure, so holding 30% of the index in cash earns what holding
    // 100% of it earns, and only cutting losses at a given exposure scores.
    // (At 0% exposure the participation factor already zeroes the part.)
    let drawdown =
        (1.0 - dd / (bench_dd.max(10.0) * (exposure / 100.0).clamp(0.01, 1.0))).clamp(0.0, 1.0);
    let discipline = if input.hard_break || input.buys.is_empty() {
        0.0
    } else {
        let n = input.buys.len() as f64;
        let stops = input.buys.iter().filter(|b| b.with_stop).count() as f64 / n;
        let sized = input
            .buys
            .iter()
            .filter(|b| b.weight_percent <= POSITION_LIMIT + 1e-9)
            .count() as f64
            / n;
        (0.6 * stops + 0.4 * sized - 0.1 * input.soft_violations as f64).max(0.0)
    };
    let diversification = 0.6 * (input.max_sectors as f64 / 5.0).min(1.0)
        + 0.4 * (input.symbols_held as f64 / 8.0).min(1.0);

    let parts = Parts {
        risk_adjusted: round(40.0 * participation * risk, 1),
        drawdown: round(20.0 * participation * drawdown, 1),
        discipline: round(25.0 * discipline, 1),
        diversification: round(15.0 * diversification, 1),
    };
    let total = parts.risk_adjusted + parts.drawdown + parts.discipline + parts.diversification;
    let points = (days >= MIN_DAYS).then(|| total.round().clamp(0.0, 100.0) as u32);
    Scorecard {
        days,
        score: points,
        grade: points.map(|p| grade(p).to_string()),
        return_percent: round(ret, 2),
        benchmark_return_percent: round(bench_ret, 2),
        max_drawdown_percent: round(dd, 2),
        benchmark_max_drawdown_percent: round(bench_dd, 2),
        sharpe: s.map(|x| round(x, 2)),
        benchmark_sharpe: bs.map(|x| round(x, 2)),
        sortino: so.map(|x| round(x, 2)),
        benchmark_sortino: bso.map(|x| round(x, 2)),
        exposure_percent: round(exposure, 1),
        parts,
    }
}

/// A disciplined run keeps the streak going: scored, at least one buy,
/// every buy with a stop and within the position limit, no rule broken.
pub fn disciplined(input: &RunInput, card: &Scorecard) -> bool {
    card.score.is_some()
        && !input.hard_break
        && input.soft_violations == 0
        && !input.buys.is_empty()
        && input
            .buys
            .iter()
            .all(|b| b.with_stop && b.weight_percent <= POSITION_LIMIT + 1e-9)
}

pub struct Badge {
    pub id: &'static str,
    pub title: Msg,
    pub how: Msg,
}

pub const BADGES: &[Badge] = &[
    Badge {
        id: "kept-your-stops",
        title: Msg::BadgeKeptStops,
        how: Msg::BadgeKeptStopsHow,
    },
    Badge {
        id: "diversified",
        title: Msg::BadgeDiversified,
        how: Msg::BadgeDiversifiedHow,
    },
    Badge {
        id: "survived-the-crash",
        title: Msg::BadgeSurvived,
        how: Msg::BadgeSurvivedHow,
    },
    Badge {
        id: "beat-the-index",
        title: Msg::BadgeBeatIndex,
        how: Msg::BadgeBeatIndexHow,
    },
    Badge {
        id: "steady-hands",
        title: Msg::BadgeSteady,
        how: Msg::BadgeSteadyHow,
    },
    Badge {
        id: "risk-adjusted",
        title: Msg::BadgeRiskAdjusted,
        how: Msg::BadgeRiskAdjustedHow,
    },
    Badge {
        id: "challenge-complete",
        title: Msg::BadgeChallenge,
        how: Msg::BadgeChallengeHow,
    },
    Badge {
        id: "sip-discipline",
        title: Msg::BadgeSip,
        how: Msg::BadgeSipHow,
    },
    Badge {
        id: "went-the-distance",
        title: Msg::BadgeDistance,
        how: Msg::BadgeDistanceHow,
    },
    Badge {
        id: "three-in-a-row",
        title: Msg::BadgeStreak,
        how: Msg::BadgeStreakHow,
    },
];

pub fn badge(id: &str) -> Option<&'static Badge> {
    BADGES.iter().find(|b| b.id == id)
}

/// Facts about a run beyond its scorecard.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunFacts {
    pub buys: usize,
    pub buys_with_stop: usize,
    pub hard_break: bool,
    pub max_sectors: usize,
    /// A challenge in this run was passed.
    pub challenge_passed: bool,
    /// A SIP challenge passed without missing a month.
    pub sip_clean: bool,
    /// Played to the scenario's last day.
    pub full_distance: bool,
}

/// Badges a run earns (the streak badge is awarded by the profile).
pub fn badges(card: &Scorecard, f: &RunFacts) -> Vec<&'static str> {
    let scored = card.score.is_some();
    let invested = |p: f64| card.exposure_percent + 1e-9 >= p;
    let mut out = Vec::new();
    let mut give = |id: &'static str, cond: bool| {
        if cond {
            out.push(id);
        }
    };
    give(
        "kept-your-stops",
        f.buys >= 3 && f.buys_with_stop == f.buys && !f.hard_break,
    );
    give("diversified", f.max_sectors >= 5);
    give(
        "survived-the-crash",
        card.days >= 20
            && card.benchmark_max_drawdown_percent >= 20.0
            && card.max_drawdown_percent <= card.benchmark_max_drawdown_percent / 2.0
            && invested(30.0),
    );
    give(
        "beat-the-index",
        scored
            && invested(30.0)
            && card.return_percent > card.benchmark_return_percent
            && card.max_drawdown_percent <= card.benchmark_max_drawdown_percent,
    );
    give(
        "steady-hands",
        card.days >= 20 && card.max_drawdown_percent < 10.0 && invested(50.0),
    );
    give(
        "risk-adjusted",
        card.days >= 20
            && invested(30.0)
            && card
                .sharpe
                .is_some_and(|s| s >= 1.0 && card.benchmark_sharpe.is_none_or(|b| s > b)),
    );
    give("challenge-complete", f.challenge_passed);
    give("sip-discipline", f.sip_clean);
    give("went-the-distance", f.full_distance && scored);
    out
}

/// `curve` rebased to 100 and thinned to at most `n` points (first and
/// last kept), for cards.
pub fn rebased(curve: &[f64], n: usize) -> Vec<f64> {
    let Some(&first) = curve.first().filter(|f| **f > 0.0) else {
        return Vec::new();
    };
    let pick: Vec<f64> = if curve.len() <= n || n < 2 {
        curve.to_vec()
    } else {
        (0..n)
            .map(|i| curve[i * (curve.len() - 1) / (n - 1)])
            .collect()
    };
    pick.iter().map(|v| round(v / first * 100.0, 2)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(nav: Vec<f64>, bench: Vec<f64>, exposure: f64) -> RunInput {
        let n = nav.len().saturating_sub(1);
        RunInput {
            nav,
            benchmark: bench,
            exposure: vec![exposure; n],
            buys: Vec::new(),
            max_sectors: 0,
            symbols_held: 0,
            hard_break: false,
            soft_violations: 0,
            periods_per_year: 252.0,
        }
    }

    /// A falling benchmark: −2% a day with a bounce every fourth day.
    fn crash(n: usize) -> Vec<f64> {
        let mut v = vec![100.0];
        for i in 1..n {
            let r = if i % 4 == 0 { 0.01 } else { -0.02 };
            v.push(v[i - 1] * (1.0 + r));
        }
        v
    }

    #[test]
    fn nav_ignores_contributions() {
        // 100 → 110 is a 10% gain; then 10 is added and equity is 120.
        let n = nav(&[100.0, 110.0, 120.0], &[0.0, 0.0, 10.0]);
        assert!((n[1] - 1.1).abs() < 1e-12);
        assert!((n[2] - 1.1).abs() < 1e-12);
        assert_eq!(nav(&[], &[]), Vec::<f64>::new());
    }

    #[test]
    fn drawdowns() {
        assert!((max_drawdown_percent(&[100.0, 120.0, 90.0, 130.0]) - 25.0).abs() < 1e-9);
        assert!((current_drawdown_percent(&[100.0, 120.0, 90.0]) - 25.0).abs() < 1e-9);
        assert_eq!(current_drawdown_percent(&[]), 0.0);
    }

    #[test]
    fn grades() {
        assert_eq!(grade(100), "A+");
        assert_eq!(grade(90), "A+");
        assert_eq!(grade(89), "A");
        assert_eq!(grade(70), "B");
        assert_eq!(grade(55), "D");
        assert_eq!(grade(12), "F");
    }

    #[test]
    fn too_short_is_not_scored() {
        let c = score(&input(vec![1.0, 1.01, 1.02], vec![100.0, 99.0, 98.0], 1.0));
        assert_eq!(c.days, 2);
        assert!(c.score.is_none() && c.grade.is_none());
    }

    #[test]
    fn sitting_in_cash_scores_low_even_in_a_crash() {
        let bench = crash(30);
        let cash = score(&input(vec![1.0; 30], bench.clone(), 0.0));
        assert_eq!(cash.parts.risk_adjusted, 0.0);
        assert_eq!(cash.parts.drawdown, 0.0);
        assert_eq!(cash.parts.discipline, 0.0);
        assert_eq!(cash.score, Some(0));
        assert_eq!(cash.grade.as_deref(), Some("F"));
    }

    /// The benchmark held at `weight` of the account, the rest in cash.
    fn blend(bench: &[f64], weight: f64) -> Vec<f64> {
        bench
            .iter()
            .map(|b| 1.0 + weight * (b / 100.0 - 1.0))
            .collect()
    }

    #[test]
    fn cash_does_not_buy_drawdown_points() {
        let bench = crash(40);
        let full = score(&input(blend(&bench, 1.0), bench.clone(), 1.0));
        let partial = score(&input(blend(&bench, 0.3), bench.clone(), 0.3));
        // Holding the index at 100% and at 30% scores the same drawdown part
        // (before: 0 against about 14 of 20).
        assert!(
            (full.parts.drawdown - partial.parts.drawdown).abs() < 1.0,
            "{} vs {}",
            full.parts.drawdown,
            partial.parts.drawdown
        );
        assert!(partial.parts.drawdown < 2.0, "{:?}", partial.parts);
    }

    #[test]
    fn a_smaller_drawdown_at_the_same_exposure_still_earns_more() {
        let bench = crash(40);
        // At 30% exposure: the index, and half the index's moves on top of
        // the same cash.
        let index = score(&input(blend(&bench, 0.3), bench.clone(), 0.3));
        let calm = score(&input(blend(&bench, 0.15), bench.clone(), 0.3));
        assert!(
            calm.parts.drawdown > index.parts.drawdown + 5.0,
            "{:?} vs {:?}",
            calm.parts,
            index.parts
        );
    }

    #[test]
    fn disciplined_defensive_run_beats_a_reckless_one() {
        let bench = crash(40);
        // Half as volatile as the index, falling half as far.
        let calm: Vec<f64> = bench
            .iter()
            .map(|b| 1.0 + (b / 100.0 - 1.0) / 2.0)
            .collect();
        let mut good = input(calm, bench.clone(), 0.6);
        good.buys = vec![
            BuyFact {
                with_stop: true,
                weight_percent: 15.0
            };
            6
        ];
        good.max_sectors = 5;
        good.symbols_held = 8;
        let g = score(&good);

        // Same index exposure, all in one stock, no stops.
        let mut bad = input(bench.iter().map(|b| b / 100.0).collect(), bench, 1.0);
        bad.buys = vec![BuyFact {
            with_stop: false,
            weight_percent: 100.0,
        }];
        bad.max_sectors = 1;
        bad.symbols_held = 1;
        let b = score(&bad);

        assert!(g.score.unwrap() > b.score.unwrap() + 30, "{g:?}\n{b:?}");
        assert_eq!(g.parts.discipline, 25.0);
        assert_eq!(g.parts.diversification, 15.0);
        assert_eq!(b.parts.discipline, 0.0);
        // Deterministic: the same input scores the same.
        assert_eq!(score(&good), g);
    }

    #[test]
    fn hard_breaks_and_soft_violations_cost_discipline() {
        let bench = crash(10);
        let mut i = input(vec![1.0; 10], bench, 0.5);
        i.buys = vec![
            BuyFact {
                with_stop: true,
                weight_percent: 10.0
            };
            2
        ];
        assert_eq!(score(&i).parts.discipline, 25.0);
        i.soft_violations = 2;
        assert_eq!(score(&i).parts.discipline, 20.0);
        i.hard_break = true;
        assert_eq!(score(&i).parts.discipline, 0.0);
    }

    #[test]
    fn badges_follow_the_rules() {
        let card = Scorecard {
            days: 60,
            score: Some(80),
            return_percent: -4.0,
            benchmark_return_percent: -25.0,
            max_drawdown_percent: 9.0,
            benchmark_max_drawdown_percent: 35.0,
            sharpe: Some(1.2),
            benchmark_sharpe: Some(-2.0),
            exposure_percent: 55.0,
            ..Scorecard::default()
        };
        let facts = RunFacts {
            buys: 4,
            buys_with_stop: 4,
            max_sectors: 5,
            full_distance: true,
            ..RunFacts::default()
        };
        assert_eq!(
            badges(&card, &facts),
            [
                "kept-your-stops",
                "diversified",
                "survived-the-crash",
                "beat-the-index",
                "steady-hands",
                "risk-adjusted",
                "went-the-distance"
            ]
        );
        // Cash all the way earns none of the performance badges.
        let idle = Scorecard {
            exposure_percent: 0.0,
            max_drawdown_percent: 0.0,
            ..card.clone()
        };
        assert!(badges(&idle, &RunFacts::default()).is_empty());
        for id in badges(&card, &facts) {
            assert!(badge(id).is_some(), "{id}");
        }
    }

    #[test]
    fn rebase_and_thin() {
        assert_eq!(rebased(&[50.0, 55.0, 60.0], 10), [100.0, 110.0, 120.0]);
        let long: Vec<f64> = (1..=100).map(f64::from).collect();
        let r = rebased(&long, 5);
        assert_eq!(r.len(), 5);
        assert_eq!(r[0], 100.0);
        assert_eq!(r[4], 10_000.0);
        assert!(rebased(&[], 5).is_empty());
    }
}
