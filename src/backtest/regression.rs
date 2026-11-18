//! Results of the bundled example strategies on fixed synthetic bars. The
//! numbers were recorded before the v2 execution options existed: a strategy
//! file that sets none of them must keep producing exactly these results.

use super::Bars;
use super::costs::CostModel;
use super::engine::{self, Asset, Fill, SimConfig, SimResult};
use super::rules;

/// 420 daily bars from a fixed-seed random walk with an upward drift, and
/// an intraday range around each bar. Synthetic; no market data.
fn bars(seed: u64) -> Bars {
    let mut seed = seed;
    let mut next = || {
        // 64-bit LCG (Knuth's MMIX constants), mapped to [-1, 1).
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let mut closes = Vec::with_capacity(420);
    let mut c = 100.0;
    for i in 0..420 {
        let x = i as f64;
        c *= 1.0 + 0.0012 + 0.03 * next() + 0.006 * (x * 0.05).sin();
        closes.push(c);
    }
    let mut b = Bars::from_closes(&closes);
    for i in 0..b.len() {
        let prev = if i == 0 { closes[0] } else { closes[i - 1] };
        b.open[i] = prev * (1.0 + 0.006 * next());
        let range = closes[i] * 0.01 * (1.0 + next().abs());
        b.high[i] = b.open[i].max(closes[i]) + range * 0.6;
        b.low[i] = b.open[i].min(closes[i]) - range * 0.4;
        b.volume[i] = 10_000 + (i as u64 * 37) % 5_000;
    }
    b
}

fn example(file: &str) -> rules::StrategyDef {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/strategies")
        .join(file);
    rules::load_file(&path).unwrap()
}

fn run(file: &str, fill: Fill) -> SimResult {
    let def = example(file);
    let b = bars(394);
    let signals = def.compile().unwrap().signals(&b).unwrap();
    let cfg = SimConfig {
        stop_loss_pct: def.stop_loss_pct,
        take_profit_pct: def.take_profit_pct,
        execution: def.execution.clone(),
        ..SimConfig::single(100_000.0, fill, CostModel::in_delivery())
    };
    engine::simulate(
        &[Asset {
            symbol: "SYN.NS",
            bars: &b,
            signals: &signals,
        }],
        &cfg,
    )
}

fn close(got: f64, want: f64) {
    assert!((got - want).abs() < 1e-6, "got {got}, want {want}");
}

struct Golden {
    end: f64,
    trades: usize,
    sharpe: f64,
    sortino: f64,
    max_drawdown: f64,
    costs: f64,
    equity_sum: f64,
    first_qty: u64,
}

fn check(r: &SimResult, g: &Golden) {
    let m = r.metrics();
    close(m.end_value, g.end);
    assert_eq!(m.trades, g.trades);
    close(m.sharpe.unwrap(), g.sharpe);
    close(m.sortino.unwrap(), g.sortino);
    close(m.max_drawdown_pct, g.max_drawdown);
    close(m.costs, g.costs);
    close(r.equity.iter().sum::<f64>(), g.equity_sum);
    assert_eq!(r.trades[0].qty, g.first_qty as f64);
}

#[test]
fn example_strategies_reproduce_their_recorded_results() {
    check(
        &run("trend_pullback.toml", Fill::NextOpen),
        &Golden {
            end: 114_560.614_510_294_05,
            trades: 1,
            sharpe: 1.029_223_035_699_140_3,
            sortino: 1.834_030_519_179_330_2,
            max_drawdown: 3.984_746_123_031_307,
            costs: 407.688_682_487_648_3,
            equity_sum: 42_563_004.212_780_49,
            first_qty: 583,
        },
    );
    check(
        &run("sma.toml", Fill::NextOpen),
        &Golden {
            end: 130_365.391_460_065_8,
            trades: 3,
            sharpe: 0.969_975_596_358_177_9,
            sortino: 1.525_777_697_571_996_3,
            max_drawdown: 16.256_895_113_627_23,
            costs: 1_362.765_296_646_452_5,
            equity_sum: 49_079_623.874_192_51,
            first_qty: 843,
        },
    );
    check(
        &run("rsi.toml", Fill::Close),
        &Golden {
            end: 108_343.964_313_319_07,
            trades: 1,
            sharpe: 0.489_953_036_396_151_2,
            sortino: 0.760_106_328_786_052_2,
            max_drawdown: 13.343_191_717_597_932,
            costs: 398.076_853_995_915_26,
            equity_sum: 43_927_612.065_264_75,
            first_qty: 831,
        },
    );
}

#[test]
fn rebalanced_portfolio_reproduces_its_recorded_result() {
    let def = rules::load("sma").unwrap();
    let s = def.compile().unwrap();
    let (a, b) = (bars(394), bars(7));
    let (sa, sb) = (s.signals(&a).unwrap(), s.signals(&b).unwrap());
    let assets = [
        Asset {
            symbol: "A.NS",
            bars: &a,
            signals: &sa,
        },
        Asset {
            symbol: "B.NS",
            bars: &b,
            signals: &sb,
        },
    ];
    let cfg = SimConfig {
        rebalance: engine::Rebalance::Monthly,
        weights: engine::Weights::Fixed(vec![0.6, 0.4]),
        ..SimConfig::single(100_000.0, Fill::NextOpen, CostModel::in_delivery())
    };
    let r = engine::simulate(&assets, &cfg);
    let m = r.metrics();
    close(m.end_value, 132_678.943_002_521_64);
    assert_eq!((m.trades, r.rebalances), (5, 13));
    close(m.sharpe.unwrap(), 1.362_944_940_840_426_2);
    close(m.costs, 1_412.204_801_351_32);
    close(r.equity.iter().sum::<f64>(), 48_785_783.816_429_98);
}

/// Old strategy files print the same JSON: no new keys on trades, costs or
/// the rules, and a whole-share quantity stays an integer.
#[test]
fn json_shape_is_unchanged_for_old_strategies() {
    let r = run("trend_pullback.toml", Fill::NextOpen);
    let t = serde_json::to_value(&r.trades[0]).unwrap();
    let keys: Vec<&str> = t.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "symbol",
            "entry_date",
            "exit_date",
            "qty",
            "entry_price",
            "exit_price",
            "gross_pnl",
            "costs",
            "net_pnl",
            "pnl_percent",
            "bars_held",
            "exit_reason"
        ]
    );
    assert_eq!(t["qty"], serde_json::json!(583));
    let c = serde_json::to_value(r.costs).unwrap();
    assert!(c.get("borrow").is_none());
    let def = serde_json::to_value(example("trend_pullback.toml")).unwrap();
    let keys: Vec<&str> = def
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "name",
            "description",
            "source",
            "entry",
            "exit",
            "stop_loss_pct",
            "take_profit_pct",
            "params",
            "grid"
        ]
    );
}

/// The v2 example runs end to end on the same bars and uses its options.
#[test]
fn trend_trail_example_uses_the_new_options() {
    let def = example("trend_trail.toml");
    assert!(!def.execution.is_default());
    let b = bars(394);
    let signals = def.compile().unwrap().signals(&b).unwrap();
    let cfg = SimConfig {
        execution: def.execution.clone(),
        ..SimConfig::single(100_000.0, Fill::NextOpen, CostModel::us())
    };
    let r = engine::simulate(
        &[Asset {
            symbol: "SYN",
            bars: &b,
            signals: &signals,
        }],
        &cfg,
    );
    assert!(r.interest > 0.0);
    assert!(r.trades.iter().any(|t| t.qty.fract() != 0.0));
    let total: f64 = r.assets.iter().map(|a| a.contribution_pct).sum();
    assert!((total - r.metrics().total_return_pct).abs() < 1e-9);
}
