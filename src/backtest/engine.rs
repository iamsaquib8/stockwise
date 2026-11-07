//! The simulator behind every backtest.
//!
//! Each symbol gets a sleeve of the capital (its weight). A sleeve is either
//! all cash or long a whole number of shares bought with all of its cash;
//! a single-symbol backtest is a portfolio of one. Signals are evaluated at
//! each bar's close and, by default, filled at the next bar's open, so a
//! trade never uses a price it could not have known. Stops and targets are
//! checked against each bar's low and high (a gap through the level fills at
//! the open; when both are touched in one bar the stop is assumed first).
//! Every fill pays slippage and the cost model's charges.

use super::costs::{Charges, CostModel, Side};
use super::rules::Signals;
use super::{Bars, date, day};
use crate::technical;
use chrono::Datelike;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fill {
    /// Signal at bar i's close, filled at bar i+1's open.
    NextOpen,
    /// Signal and fill at bar i's close (optimistic: the close is only known
    /// once it has happened).
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExitReason {
    Signal,
    StopLoss,
    TakeProfit,
    Rebalance,
    EndOfData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Rebalance {
    None,
    Monthly,
    Quarterly,
}

impl Rebalance {
    /// Whether the first bar of `day` starts a new period after `prev`.
    fn due(self, prev: i64, day: i64) -> bool {
        let d = |x: i64| chrono::DateTime::from_timestamp(x * 86_400, 0).map(|t| t.date_naive());
        let (Some(a), Some(b)) = (d(prev), d(day)) else {
            return false;
        };
        match self {
            Rebalance::None => false,
            Rebalance::Monthly => (a.year(), a.month()) != (b.year(), b.month()),
            Rebalance::Quarterly => (a.year(), a.month0() / 3) != (b.year(), b.month0() / 3),
        }
    }
}

/// How capital is split across symbols.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Weights {
    Equal,
    /// Weight ∝ 1 / volatility of the trailing 63 daily returns, recomputed at
    /// each rebalance.
    InverseVol,
    /// Explicit weights, normalised to sum to 1.
    Fixed(Vec<f64>),
}

impl Weights {
    /// Parse `equal`, `inverse-vol`, or a list like `0.5,0.3,0.2`.
    pub fn parse(s: &str) -> Result<Weights, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "equal" => return Ok(Weights::Equal),
            "inverse-vol" | "inverse_vol" | "inv-vol" | "invvol" => {
                return Ok(Weights::InverseVol);
            }
            _ => {}
        }
        let w: Vec<f64> = s
            .split(',')
            .map(|p| p.trim().trim_end_matches('%').parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("`{s}`: use equal, inverse-vol, or weights like 0.5,0.3,0.2"))?;
        if w.iter().any(|x| !x.is_finite() || *x < 0.0) || w.iter().sum::<f64>() <= 0.0 {
            return Err(format!(
                "`{s}`: weights must be zero or more, and not all zero"
            ));
        }
        Ok(Weights::Fixed(w))
    }

    /// Starting weights for `n` symbols (inverse-vol starts equal: there is
    /// no history yet).
    pub fn initial(&self, n: usize) -> Result<Vec<f64>, String> {
        match self {
            Weights::Equal | Weights::InverseVol => Ok(vec![1.0 / n as f64; n]),
            Weights::Fixed(w) if w.len() != n => Err(format!(
                "{} weights given for {n} symbols — give one weight per symbol",
                w.len()
            )),
            Weights::Fixed(w) => {
                let sum: f64 = w.iter().sum();
                Ok(w.iter().map(|x| x / sum).collect())
            }
        }
    }
}

/// One symbol's input to [`simulate`].
pub struct Asset<'a> {
    pub symbol: &'a str,
    pub bars: &'a Bars,
    pub signals: &'a Signals,
}

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub capital: f64,
    pub fill: Fill,
    pub costs: CostModel,
    pub stop_loss_pct: Option<f64>,
    pub take_profit_pct: Option<f64>,
    pub weights: Weights,
    pub rebalance: Rebalance,
    /// Bars in a year, for annualising Sharpe and Sortino (see
    /// [`bars_per_year`]).
    pub periods_per_year: f64,
}

/// Daily bars in a year: 252 trading days for stocks, 365 for crypto (it
/// trades every day), 260 for FX (weekdays). A portfolio uses the most
/// frequent of its symbols, since its calendar is the union of theirs.
pub fn bars_per_year<'a>(symbols: impl IntoIterator<Item = &'a str>) -> f64 {
    use crate::market::AssetClass;
    symbols
        .into_iter()
        .map(|s| match AssetClass::of(s) {
            AssetClass::Crypto => 365.0,
            AssetClass::Fx => 260.0,
            AssetClass::Stock | AssetClass::Index => 252.0,
        })
        .fold(252.0, f64::max)
}

impl SimConfig {
    /// One symbol, all capital, no rebalancing.
    pub fn single(capital: f64, fill: Fill, costs: CostModel) -> SimConfig {
        SimConfig {
            capital,
            fill,
            costs,
            stop_loss_pct: None,
            take_profit_pct: None,
            weights: Weights::Equal,
            rebalance: Rebalance::None,
            periods_per_year: 252.0,
        }
    }
}

/// A round trip: from flat to flat.
#[derive(Debug, Clone, Serialize)]
pub struct Trade {
    pub symbol: String,
    pub entry_date: String,
    pub exit_date: String,
    /// Shares bought on entry.
    pub qty: u64,
    /// Average entry price, before slippage.
    pub entry_price: f64,
    /// Price of the closing sell, before slippage.
    pub exit_price: f64,
    pub gross_pnl: f64,
    /// Charges plus slippage, both legs.
    pub costs: f64,
    pub net_pnl: f64,
    /// Net P&L as a percent of the money put in.
    #[serde(rename = "pnl_percent")]
    pub return_pct: f64,
    pub bars_held: usize,
    pub exit_reason: ExitReason,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct CostTotals {
    #[serde(flatten)]
    pub charges: Charges,
    pub slippage: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetResult {
    pub symbol: String,
    #[serde(rename = "weight")]
    pub initial_weight: f64,
    /// Net profit of this sleeve, after costs.
    pub pnl: f64,
    /// `pnl` as a percent of the starting capital; these add up to the
    /// portfolio's total return.
    #[serde(rename = "contribution_percent")]
    pub contribution_pct: f64,
    pub trades: usize,
    #[serde(rename = "win_rate_percent")]
    pub win_rate_pct: Option<f64>,
    pub costs: f64,
    /// Percent of days this symbol was held.
    #[serde(rename = "exposure_percent")]
    pub exposure_pct: f64,
}

#[derive(Debug, Clone)]
pub struct SimResult {
    pub capital: f64,
    pub periods_per_year: f64,
    /// Calendar days (UTC midnight) on which any symbol traded.
    pub times: Vec<i64>,
    /// Portfolio value at each day's close (after liquidation on the last day).
    pub equity: Vec<f64>,
    /// Value of open positions at each day's close.
    pub invested: Vec<f64>,
    pub trades: Vec<Trade>,
    pub assets: Vec<AssetResult>,
    pub costs: CostTotals,
    /// Entry signals that could not buy a single share.
    pub skipped_entries: usize,
    pub rebalances: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    pub start_value: f64,
    pub end_value: f64,
    #[serde(rename = "total_return_percent")]
    pub total_return_pct: f64,
    /// Compound annual growth; `None` under three months of data.
    #[serde(rename = "cagr_percent")]
    pub cagr_pct: Option<f64>,
    #[serde(rename = "max_drawdown_percent")]
    pub max_drawdown_pct: f64,
    /// Annualised from daily returns, risk-free rate 0.
    pub sharpe: Option<f64>,
    pub sortino: Option<f64>,
    /// Average share of the portfolio held in stocks rather than cash.
    #[serde(rename = "exposure_percent")]
    pub exposure_pct: f64,
    #[serde(rename = "trade_count")]
    pub trades: usize,
    #[serde(rename = "win_rate_percent")]
    pub win_rate_pct: Option<f64>,
    #[serde(rename = "avg_win_percent")]
    pub avg_win_pct: Option<f64>,
    #[serde(rename = "avg_loss_percent")]
    pub avg_loss_pct: Option<f64>,
    /// Money won on winning trades ÷ money lost on losing ones, after costs.
    pub profit_factor: Option<f64>,
    /// Charges plus slippage.
    #[serde(rename = "costs_paid")]
    pub costs: f64,
}

impl SimResult {
    pub fn metrics(&self) -> Metrics {
        metrics(
            self.capital,
            &self.times,
            &self.equity,
            &self.invested,
            &self.trades,
            self.costs.total,
            self.periods_per_year,
        )
    }
}

/// Performance metrics for an equity curve that started at `start`.
pub fn metrics(
    start: f64,
    times: &[i64],
    equity: &[f64],
    invested: &[f64],
    trades: &[Trade],
    costs: f64,
    periods_per_year: f64,
) -> Metrics {
    let end = equity.last().copied().unwrap_or(start);
    let mut curve = Vec::with_capacity(equity.len() + 1);
    curve.push(start);
    curve.extend_from_slice(equity);
    let years = match (times.first(), times.last()) {
        (Some(a), Some(b)) => (b - a) as f64 / (365.25 * 86_400.0),
        _ => 0.0,
    };
    let cagr_pct = (years >= 0.25 && start > 0.0 && end > 0.0)
        .then(|| ((end / start).powf(1.0 / years) - 1.0) * 100.0);
    let exposure_pct = if equity.is_empty() {
        0.0
    } else {
        equity
            .iter()
            .zip(invested)
            .map(|(e, i)| if *e > 0.0 { i / e } else { 0.0 })
            .sum::<f64>()
            / equity.len() as f64
            * 100.0
    };

    let wins: Vec<&Trade> = trades.iter().filter(|t| t.net_pnl > 0.0).collect();
    let losses: Vec<&Trade> = trades.iter().filter(|t| t.net_pnl <= 0.0).collect();
    let mean = |ts: &[&Trade]| {
        (!ts.is_empty()).then(|| ts.iter().map(|t| t.return_pct).sum::<f64>() / ts.len() as f64)
    };
    // fold from +0.0: an empty f64 `sum()` is -0.0, which prints as "-0.00".
    let won = wins.iter().fold(0.0, |a, t| a + t.net_pnl);
    let lost = losses.iter().fold(0.0, |a, t| a - t.net_pnl);

    Metrics {
        start_value: start,
        end_value: end,
        total_return_pct: (end / start - 1.0) * 100.0,
        cagr_pct,
        max_drawdown_pct: technical::max_drawdown(&curve)
            .map(|(dd, _, _)| dd * 100.0)
            .unwrap_or(0.0),
        sharpe: sharpe(&curve, periods_per_year),
        sortino: sortino(&curve, periods_per_year),
        exposure_pct,
        trades: trades.len(),
        win_rate_pct: (!trades.is_empty()).then(|| wins.len() as f64 / trades.len() as f64 * 100.0),
        avg_win_pct: mean(&wins),
        avg_loss_pct: mean(&losses),
        profit_factor: (lost > 0.0).then(|| won / lost),
        costs,
    }
}

/// Annualise a return earned over `bars` bars.
pub fn annualised_pct(return_pct: f64, bars: usize, periods_per_year: f64) -> Option<f64> {
    let growth = 1.0 + return_pct / 100.0;
    (bars > 0 && growth > 0.0).then(|| (growth.powf(periods_per_year / bars as f64) - 1.0) * 100.0)
}

/// `technical::sharpe_ratio` (risk-free 0) with a chosen bars-per-year.
pub(crate) fn sharpe(curve: &[f64], periods_per_year: f64) -> Option<f64> {
    let r = technical::daily_returns(curve);
    if r.len() < 2 {
        return None;
    }
    let mean = r.iter().sum::<f64>() / r.len() as f64;
    let var = r.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (r.len() - 1) as f64;
    let sd = var.sqrt();
    (sd != 0.0).then(|| mean * periods_per_year / (sd * periods_per_year.sqrt()))
}

/// `technical::sortino_ratio` (risk-free 0) with a chosen bars-per-year.
/// Delegates so the downside deviation (taken over all periods) is defined once.
pub(crate) fn sortino(curve: &[f64], periods_per_year: f64) -> Option<f64> {
    technical::sortino_ratio_with(curve, 0.0, periods_per_year)
}

// ── Simulation ──

struct Position {
    qty: u64,
    /// Average entry price before slippage — stops and targets key off it.
    avg: f64,
    entry_bar: usize,
    entry_time: i64,
    entry_qty: u64,
    /// Gross profit already realised by partial sells (rebalancing).
    gross: f64,
    costs: f64,
    /// Money put in: shares × raw price, summed over every buy.
    basis: f64,
}

struct Sleeve {
    cash: f64,
    pos: Option<Position>,
    pending_entry: bool,
    pending_exit: bool,
    /// Index of the next bar to process.
    cursor: usize,
    last_close: Option<f64>,
    initial: f64,
    transfers: f64,
    trades: Vec<Trade>,
    costs: f64,
    days_held: usize,
    days_listed: usize,
}

struct Sim<'a> {
    assets: &'a [Asset<'a>],
    cfg: &'a SimConfig,
    sleeves: Vec<Sleeve>,
    costs: CostTotals,
    skipped: usize,
}

impl Sim<'_> {
    fn buy(&mut self, k: usize, bar: usize, raw: f64, top_up: bool) {
        let model = &self.cfg.costs;
        let px = model.fill_price(Side::Buy, raw);
        let s = &mut self.sleeves[k];
        let cost = |q: u64| {
            let t = q as f64 * px;
            t + model.charges(Side::Buy, t).total()
        };
        let room = s.cash - model.charges(Side::Buy, s.cash.max(0.0)).total();
        let mut qty = if px > 0.0 && room > 0.0 {
            (room / px).floor() as u64
        } else {
            0
        };
        while let Some(next) = qty.checked_add(1)
            && cost(next) <= s.cash
        {
            qty = next;
        }
        while qty > 0 && cost(qty) > s.cash {
            qty -= 1;
        }
        if qty == 0 {
            if !top_up {
                self.skipped += 1;
            }
            return;
        }
        let turnover = qty as f64 * px;
        let ch = model.charges(Side::Buy, turnover);
        let slip = qty as f64 * (px - raw);
        s.cash -= turnover + ch.total();
        s.costs += ch.total() + slip;
        self.costs.charges.add(&ch);
        self.costs.slippage += slip;
        match &mut s.pos {
            Some(p) => {
                p.avg = (p.avg * p.qty as f64 + raw * qty as f64) / (p.qty + qty) as f64;
                p.qty += qty;
                p.costs += ch.total() + slip;
                p.basis += raw * qty as f64;
            }
            None => {
                s.pos = Some(Position {
                    qty,
                    avg: raw,
                    entry_bar: bar,
                    entry_time: self.assets[k].bars.time[bar],
                    entry_qty: qty,
                    gross: 0.0,
                    costs: ch.total() + slip,
                    basis: raw * qty as f64,
                });
            }
        }
    }

    fn sell(&mut self, k: usize, bar: usize, raw: f64, qty: Option<u64>, reason: ExitReason) {
        let model = &self.cfg.costs;
        let s = &mut self.sleeves[k];
        let Some(p) = s.pos.as_mut() else {
            return;
        };
        let qty = qty.unwrap_or(p.qty).min(p.qty);
        if qty == 0 {
            return;
        }
        let px = model.fill_price(Side::Sell, raw);
        let turnover = qty as f64 * px;
        let ch = model.charges(Side::Sell, turnover);
        let slip = qty as f64 * (raw - px);
        s.cash += turnover - ch.total();
        s.costs += ch.total() + slip;
        self.costs.charges.add(&ch);
        self.costs.slippage += slip;
        p.gross += qty as f64 * (raw - p.avg);
        p.costs += ch.total() + slip;
        p.qty -= qty;
        if p.qty > 0 {
            return;
        }
        let p = s.pos.take().expect("position checked above");
        let net = p.gross - p.costs;
        let asset = &self.assets[k];
        s.trades.push(Trade {
            symbol: asset.symbol.to_string(),
            entry_date: date(p.entry_time),
            exit_date: date(asset.bars.time[bar]),
            qty: p.entry_qty,
            entry_price: p.avg,
            exit_price: raw,
            gross_pnl: p.gross,
            costs: p.costs,
            net_pnl: net,
            return_pct: if p.basis > 0.0 {
                net / p.basis * 100.0
            } else {
                0.0
            },
            bars_held: bar - p.entry_bar,
            exit_reason: reason,
        });
    }

    /// Orders queued at yesterday's close fill at today's open.
    fn on_open(&mut self, k: usize, bar: usize) {
        let open = self.assets[k].bars.open[bar];
        let s = &mut self.sleeves[k];
        let (entry, exit) = (s.pending_entry, s.pending_exit);
        s.pending_entry = false;
        s.pending_exit = false;
        if exit {
            self.sell(k, bar, open, None, ExitReason::Signal);
        }
        if entry && self.sleeves[k].pos.is_none() {
            self.buy(k, bar, open, false);
        }
    }

    /// Stop and target exits. With `at_open`, only gaps through a level at
    /// the open (checked before a rebalance trades at that open); otherwise
    /// also touches during the bar.
    fn check_stops(&mut self, k: usize, bar: usize, at_open: bool) {
        let b = self.assets[k].bars;
        let (o, h, l) = (b.open[bar], b.high[bar], b.low[bar]);
        let Some(p) = &self.sleeves[k].pos else {
            return;
        };
        // A position opened at this bar's open can't gap through its own level.
        let gap = p.entry_bar < bar;
        if let Some(sl) = self.cfg.stop_loss_pct {
            let stop = p.avg * (1.0 - sl / 100.0);
            if gap && o <= stop {
                return self.sell(k, bar, o, None, ExitReason::StopLoss);
            }
            if !at_open && l <= stop {
                return self.sell(k, bar, stop, None, ExitReason::StopLoss);
            }
        }
        if let Some(tp) = self.cfg.take_profit_pct {
            let target = p.avg * (1.0 + tp / 100.0);
            if gap && o >= target {
                return self.sell(k, bar, o, None, ExitReason::TakeProfit);
            }
            if !at_open && h >= target {
                self.sell(k, bar, target, None, ExitReason::TakeProfit);
            }
        }
    }

    fn on_close(&mut self, k: usize, bar: usize) {
        let a = &self.assets[k];
        let (entry, exit) = (a.signals.entry[bar], a.signals.exit[bar]);
        let close = a.bars.close[bar];
        let holding = self.sleeves[k].pos.is_some();
        match self.cfg.fill {
            Fill::Close => {
                if holding && exit {
                    self.sell(k, bar, close, None, ExitReason::Signal);
                } else if !holding && entry && !exit {
                    self.buy(k, bar, close, false);
                }
            }
            Fill::NextOpen => {
                let s = &mut self.sleeves[k];
                s.pending_exit = holding && exit;
                s.pending_entry = !holding && entry && !exit;
            }
        }
    }

    fn target_weights(&self) -> Vec<f64> {
        let n = self.assets.len();
        let equal = vec![1.0 / n as f64; n];
        match &self.cfg.weights {
            Weights::InverseVol => {
                let inv: Option<Vec<f64>> = self
                    .assets
                    .iter()
                    .zip(&self.sleeves)
                    .map(|(a, s)| {
                        // Closes strictly before today: no look-ahead.
                        let closes = &a.bars.close[s.cursor.saturating_sub(64)..s.cursor];
                        let vol = technical::annualized_volatility(closes)?;
                        (closes.len() > 21 && vol > 0.0).then_some(1.0 / vol)
                    })
                    .collect();
                match inv {
                    Some(inv) => {
                        let sum: f64 = inv.iter().sum();
                        inv.iter().map(|x| x / sum).collect()
                    }
                    None => equal,
                }
            }
            w => w.initial(n).unwrap_or(equal),
        }
    }

    /// Bring every sleeve back to its target weight at today's open.
    fn rebalance(&mut self, today: &[Option<usize>]) {
        let n = self.assets.len();
        let price = |sim: &Self, k: usize| {
            today[k]
                .map(|b| sim.assets[k].bars.open[b])
                .or(sim.sleeves[k].last_close)
                .unwrap_or(0.0)
        };
        let held = |sim: &Self, k: usize| sim.sleeves[k].pos.as_ref().map_or(0, |p| p.qty) as f64;
        let value = |sim: &Self, k: usize| sim.sleeves[k].cash + held(sim, k) * price(sim, k);
        let total: f64 = (0..n).map(|k| value(self, k)).sum();
        let w = self.target_weights();
        let target: Vec<f64> = w.iter().map(|w| w * total).collect();

        // Trim positions worth more than their target (ignore drifts under 1%
        // of the target). Idle cash moves in the next step, not by selling.
        for k in 0..n {
            let Some(bar) = today[k] else { continue };
            let px = price(self, k);
            let excess = held(self, k) * px - target[k];
            if held(self, k) > 0.0 && px > 0.0 && excess > 0.01 * target[k] {
                let qty = (excess / px).floor() as u64;
                self.sell(k, bar, px, Some(qty), ExitReason::Rebalance);
            }
        }
        // Move cash so each sleeve's value matches its target.
        let pool: f64 = self.sleeves.iter().map(|s| s.cash).sum();
        let want: Vec<f64> = (0..n)
            .map(|k| (target[k] - held(self, k) * price(self, k)).max(0.0))
            .collect();
        let want_sum: f64 = want.iter().sum();
        let scale = if want_sum > pool && want_sum > 0.0 {
            pool / want_sum
        } else {
            1.0
        };
        let leftover = pool - want_sum * scale;
        for k in 0..n {
            let cash = want[k] * scale + leftover * w[k];
            let s = &mut self.sleeves[k];
            s.transfers += cash - s.cash;
            s.cash = cash;
        }
        // Top up underweight positions with their new cash.
        for k in 0..n {
            let Some(bar) = today[k] else { continue };
            if held(self, k) > 0.0 && self.sleeves[k].cash > 0.01 * target[k] {
                self.buy(k, bar, price(self, k), true);
            }
        }
    }
}

/// Run `assets` through the strategy signals they carry.
pub fn simulate(assets: &[Asset], cfg: &SimConfig) -> SimResult {
    let n = assets.len();
    let weights = cfg
        .weights
        .initial(n)
        .unwrap_or_else(|_| vec![1.0 / n.max(1) as f64; n]);
    let sleeves = weights
        .iter()
        .map(|w| Sleeve {
            cash: cfg.capital * w,
            pos: None,
            pending_entry: false,
            pending_exit: false,
            cursor: 0,
            last_close: None,
            initial: cfg.capital * w,
            transfers: 0.0,
            trades: Vec::new(),
            costs: 0.0,
            days_held: 0,
            days_listed: 0,
        })
        .collect();
    let mut sim = Sim {
        assets,
        cfg,
        sleeves,
        costs: CostTotals::default(),
        skipped: 0,
    };

    let mut days: Vec<i64> = assets
        .iter()
        .flat_map(|a| a.bars.time.iter().map(|t| day(*t)))
        .collect();
    days.sort_unstable();
    days.dedup();

    let (mut times, mut equity, mut invested) = (Vec::new(), Vec::new(), Vec::new());
    let mut rebalances = 0;
    for (di, &d) in days.iter().enumerate() {
        let today: Vec<Option<usize>> = (0..n)
            .map(|k| {
                let c = sim.sleeves[k].cursor;
                (c < assets[k].bars.len() && day(assets[k].bars.time[c]) == d).then_some(c)
            })
            .collect();
        for k in 0..n {
            if let Some(bar) = today[k] {
                sim.on_open(k, bar);
            }
        }
        if di > 0 && cfg.rebalance.due(days[di - 1], d) && n > 1 {
            // A stop gapped through at the open fills before the rebalance.
            for k in 0..n {
                if let Some(bar) = today[k] {
                    sim.check_stops(k, bar, true);
                }
            }
            sim.rebalance(&today);
            rebalances += 1;
        }
        for k in 0..n {
            if let Some(bar) = today[k] {
                sim.check_stops(k, bar, false);
                sim.on_close(k, bar);
                let s = &mut sim.sleeves[k];
                s.last_close = Some(assets[k].bars.close[bar]);
                s.cursor += 1;
            }
        }
        let (mut eq, mut inv) = (0.0, 0.0);
        for s in &mut sim.sleeves {
            let held = s
                .pos
                .as_ref()
                .map_or(0.0, |p| p.qty as f64 * s.last_close.unwrap_or(0.0));
            eq += s.cash + held;
            inv += held;
            if s.last_close.is_some() {
                s.days_listed += 1;
                s.days_held += (held > 0.0) as usize;
            }
        }
        times.push(d * 86_400);
        equity.push(eq);
        invested.push(inv);
    }

    // Close whatever is still open at each symbol's last close.
    for k in 0..n {
        if sim.sleeves[k].pos.is_some() {
            let last = assets[k].bars.len() - 1;
            sim.sell(
                k,
                last,
                assets[k].bars.close[last],
                None,
                ExitReason::EndOfData,
            );
        }
    }
    if let Some(e) = equity.last_mut() {
        *e = sim.sleeves.iter().map(|s| s.cash).sum();
        if let Some(i) = invested.last_mut() {
            *i = 0.0;
        }
    }

    let mut trades = Vec::new();
    let mut results = Vec::new();
    for (k, s) in sim.sleeves.iter().enumerate() {
        let pnl = s.cash - s.initial - s.transfers;
        let wins = s.trades.iter().filter(|t| t.net_pnl > 0.0).count();
        results.push(AssetResult {
            symbol: assets[k].symbol.to_string(),
            initial_weight: weights[k],
            pnl,
            contribution_pct: pnl / cfg.capital * 100.0,
            trades: s.trades.len(),
            win_rate_pct: (!s.trades.is_empty())
                .then(|| wins as f64 / s.trades.len() as f64 * 100.0),
            costs: s.costs,
            exposure_pct: if s.days_listed > 0 {
                s.days_held as f64 / s.days_listed as f64 * 100.0
            } else {
                0.0
            },
        });
        trades.extend(s.trades.iter().cloned());
    }
    trades.sort_by(|a, b| a.exit_date.cmp(&b.exit_date));
    let mut costs = sim.costs;
    costs.total = costs.charges.total() + costs.slippage;

    SimResult {
        capital: cfg.capital,
        periods_per_year: cfg.periods_per_year,
        times,
        equity,
        invested,
        trades,
        assets: results,
        costs,
        skipped_entries: sim.skipped,
        rebalances,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(n: usize, entries: &[usize], exits: &[usize]) -> Signals {
        let mut s = Signals {
            entry: vec![false; n],
            exit: vec![false; n],
        };
        entries.iter().for_each(|&i| s.entry[i] = true);
        exits.iter().for_each(|&i| s.exit[i] = true);
        s
    }

    fn bars(opens: &[f64], closes: &[f64]) -> Bars {
        let mut b = Bars::from_closes(closes);
        b.open = opens.to_vec();
        for i in 0..b.len() {
            b.high[i] = opens[i].max(closes[i]);
            b.low[i] = opens[i].min(closes[i]);
        }
        b
    }

    fn run(b: &Bars, s: &Signals, cfg: &SimConfig) -> SimResult {
        simulate(
            &[Asset {
                symbol: "T",
                bars: b,
                signals: s,
            }],
            cfg,
        )
    }

    #[test]
    fn next_open_fill_uses_the_following_bar() {
        let b = bars(&[10.0, 11.0, 12.0, 13.0], &[10.5, 11.5, 12.5, 13.5]);
        let s = signals(4, &[0], &[2]);
        let r = run(
            &b,
            &s,
            &SimConfig::single(1_000.0, Fill::NextOpen, CostModel::none()),
        );
        let t = &r.trades[0];
        assert_eq!((t.entry_price, t.exit_price), (11.0, 13.0)); // opens of bars 1 and 3
        assert_eq!(t.qty, 90); // floor(1000 / 11)
        assert!((t.net_pnl - 180.0).abs() < 1e-9);
        assert!((r.equity[3] - 1_180.0).abs() < 1e-9);
        assert_eq!(t.exit_reason, ExitReason::Signal);
    }

    #[test]
    fn close_fill_uses_the_signal_bar() {
        let b = bars(&[10.0, 11.0, 12.0, 13.0], &[10.0, 11.5, 12.5, 13.5]);
        let s = signals(4, &[0], &[2]);
        let r = run(
            &b,
            &s,
            &SimConfig::single(1_000.0, Fill::Close, CostModel::none()),
        );
        let t = &r.trades[0];
        assert_eq!((t.entry_price, t.exit_price), (10.0, 12.5));
        assert_eq!(t.bars_held, 2);
    }

    #[test]
    fn open_position_is_closed_at_the_end_and_charged() {
        let b = bars(&[100.0; 3], &[100.0, 100.0, 110.0]);
        let s = signals(3, &[0], &[]);
        let cfg = SimConfig::single(100_000.0, Fill::NextOpen, CostModel::in_delivery());
        let r = run(&b, &s, &cfg);
        assert_eq!(r.trades.len(), 1);
        let t = &r.trades[0];
        assert_eq!(t.exit_reason, ExitReason::EndOfData);
        assert!(t.costs > 0.0);
        assert!((t.net_pnl - (t.gross_pnl - t.costs)).abs() < 1e-6);
        // Equity ends as cash and agrees with the trade.
        assert!((r.equity[2] - (100_000.0 + t.net_pnl)).abs() < 1e-6);
        assert!(r.invested[2] == 0.0);
        assert!((r.costs.total - t.costs).abs() < 1e-6);
    }

    #[test]
    fn costs_lower_the_result_and_cash_never_goes_negative() {
        let b = bars(&[100.0, 100.0, 100.0], &[100.0, 100.0, 100.0]);
        let s = signals(3, &[0], &[1]);
        let r = run(
            &b,
            &s,
            &SimConfig::single(10_000.0, Fill::Close, CostModel::in_delivery()),
        );
        let t = &r.trades[0];
        // Buying 100 shares would leave nothing for charges: 99 it is.
        assert_eq!(t.qty, 99);
        assert!(t.net_pnl < 0.0);
        assert!(r.equity.iter().all(|e| *e > 0.0 && *e < 10_000.0 + 1e-9));
    }

    #[test]
    fn stop_loss_fills_at_the_stop_or_the_gap_open() {
        // Entry at 100 (bar 1 open). Bar 2 trades down to 90 → 5% stop at 95.
        let mut b = bars(&[100.0, 100.0, 99.0, 99.0], &[100.0, 100.0, 96.0, 99.0]);
        b.low[2] = 90.0;
        let s = signals(4, &[0], &[]);
        let mut cfg = SimConfig::single(10_000.0, Fill::NextOpen, CostModel::none());
        cfg.stop_loss_pct = Some(5.0);
        let r = run(&b, &s, &cfg);
        assert_eq!(r.trades[0].exit_reason, ExitReason::StopLoss);
        assert!((r.trades[0].exit_price - 95.0).abs() < 1e-9);

        // Gap down through the stop: filled at the (worse) open.
        let b = bars(&[100.0, 100.0, 90.0, 90.0], &[100.0, 100.0, 92.0, 92.0]);
        let r = run(&b, &s, &cfg);
        assert!((r.trades[0].exit_price - 90.0).abs() < 1e-9);
    }

    #[test]
    fn take_profit_and_stop_in_one_bar_assumes_the_stop() {
        let mut b = bars(&[100.0, 100.0, 100.0], &[100.0, 100.0, 100.0]);
        b.high[2] = 120.0;
        b.low[2] = 80.0;
        let s = signals(3, &[0], &[]);
        let mut cfg = SimConfig::single(10_000.0, Fill::NextOpen, CostModel::none());
        cfg.stop_loss_pct = Some(5.0);
        cfg.take_profit_pct = Some(10.0);
        let r = run(&b, &s, &cfg);
        assert_eq!(r.trades[0].exit_reason, ExitReason::StopLoss);
    }

    #[test]
    fn entry_signal_on_the_last_bar_never_fills() {
        let b = bars(&[10.0, 10.0], &[10.0, 10.0]);
        let s = signals(2, &[1], &[]);
        let r = run(
            &b,
            &s,
            &SimConfig::single(1_000.0, Fill::NextOpen, CostModel::none()),
        );
        assert!(r.trades.is_empty());
        assert_eq!(r.equity, vec![1_000.0, 1_000.0]);
    }

    #[test]
    fn too_little_capital_skips_the_entry() {
        let b = bars(&[500.0, 500.0, 500.0], &[500.0, 500.0, 500.0]);
        let s = signals(3, &[0], &[]);
        let r = run(
            &b,
            &s,
            &SimConfig::single(100.0, Fill::NextOpen, CostModel::none()),
        );
        assert!(r.trades.is_empty());
        assert_eq!(r.skipped_entries, 1);
    }

    #[test]
    fn metrics_from_daily_equity() {
        let b = bars(
            &[100.0, 100.0, 110.0, 99.0, 120.0],
            &[100.0, 110.0, 99.0, 120.0, 120.0],
        );
        let s = signals(5, &[0], &[]);
        let r = run(
            &b,
            &s,
            &SimConfig::single(1_000.0, Fill::NextOpen, CostModel::none()),
        );
        let m = r.metrics();
        assert!((m.total_return_pct - 20.0).abs() < 1e-9);
        assert!((m.max_drawdown_pct - 10.0).abs() < 1e-9); // 110 → 99
        assert!(m.sharpe.is_some());
        assert_eq!(m.trades, 1);
        assert_eq!(m.win_rate_pct, Some(100.0));
        assert!(m.cagr_pct.is_none()); // five days is too short to annualise
        assert!(m.exposure_pct > 50.0);
    }

    #[test]
    fn portfolio_sleeves_contributions_add_up() {
        let up = bars(&[100.0, 100.0, 110.0, 120.0], &[100.0, 110.0, 120.0, 130.0]);
        let down = bars(&[100.0, 100.0, 90.0, 80.0], &[100.0, 90.0, 80.0, 70.0]);
        let s = signals(4, &[0], &[]);
        let assets = [
            Asset {
                symbol: "UP",
                bars: &up,
                signals: &s,
            },
            Asset {
                symbol: "DOWN",
                bars: &down,
                signals: &s,
            },
        ];
        let mut cfg = SimConfig::single(2_000.0, Fill::NextOpen, CostModel::in_delivery());
        cfg.weights = Weights::Fixed(vec![3.0, 1.0]);
        let r = simulate(&assets, &cfg);
        assert_eq!(r.assets[0].initial_weight, 0.75);
        let total: f64 = r.assets.iter().map(|a| a.contribution_pct).sum();
        let m = r.metrics();
        assert!((total - m.total_return_pct).abs() < 1e-9);
        assert!(r.assets[0].pnl > 0.0 && r.assets[1].pnl < 0.0);
    }

    #[test]
    fn monthly_rebalance_trims_winners_and_tops_up_laggards() {
        // From 2024-01-01: A doubles during January, B is flat until February
        // and then rises ₹1 a day. Both are bought at ₹100 on 2 January.
        let price_a = |i: usize| {
            if i <= 1 {
                100.0
            } else {
                (100.0 + (i - 1) as f64 * 100.0 / 29.0).min(200.0)
            }
        };
        let price_b = |i: usize| 100.0 + i.saturating_sub(31) as f64;
        let mk = |f: &dyn Fn(usize) -> f64| {
            let closes: Vec<f64> = (0..40).map(f).collect();
            Bars::from_closes(&closes)
        };
        let (a, b) = (mk(&price_a), mk(&price_b));
        let s = signals(40, &[0], &[]);
        let assets = [
            Asset {
                symbol: "A",
                bars: &a,
                signals: &s,
            },
            Asset {
                symbol: "B",
                bars: &b,
                signals: &s,
            },
        ];
        let mut cfg = SimConfig::single(100_000.0, Fill::NextOpen, CostModel::none());
        cfg.rebalance = Rebalance::Monthly;
        let r = simulate(&assets, &cfg);
        assert_eq!(r.rebalances, 1);
        // 1 Feb: A is worth ₹1,00,000 and B ₹50,000. A sells 125 shares and
        // B buys 250 with the cash, so B then holds 750 shares (not 500).
        assert!((r.assets[0].pnl - 50_000.0).abs() < 1e-6);
        assert!((r.assets[1].pnl - 750.0 * 8.0).abs() < 1e-6);
        assert!((r.equity[39] - 156_000.0).abs() < 1e-6);
        let total: f64 = r.assets.iter().map(|x| x.contribution_pct).sum();
        assert!((total - r.metrics().total_return_pct).abs() < 1e-9);

        // Without rebalancing B keeps its 500 shares.
        cfg.rebalance = Rebalance::None;
        let r = simulate(&assets, &cfg);
        assert!((r.assets[1].pnl - 500.0 * 8.0).abs() < 1e-6);
    }

    #[test]
    fn weights_parse_and_validate() {
        assert_eq!(Weights::parse("equal").unwrap(), Weights::Equal);
        assert_eq!(Weights::parse("Inverse-Vol").unwrap(), Weights::InverseVol);
        assert_eq!(
            Weights::parse("0.5, 0.3,0.2").unwrap(),
            Weights::Fixed(vec![0.5, 0.3, 0.2])
        );
        assert!(Weights::parse("0,0").is_err());
        assert!(Weights::parse("a,b").is_err());
        assert!(Weights::Fixed(vec![1.0, 1.0]).initial(3).is_err());
        assert_eq!(
            Weights::Fixed(vec![1.0, 3.0]).initial(2).unwrap(),
            vec![0.25, 0.75]
        );
    }

    #[test]
    fn rebalance_due_dates() {
        let d = |y, m, dd| {
            chrono::NaiveDate::from_ymd_opt(y, m, dd)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
                / 86_400
        };
        assert!(Rebalance::Monthly.due(d(2024, 1, 31), d(2024, 2, 1)));
        assert!(!Rebalance::Monthly.due(d(2024, 2, 1), d(2024, 2, 2)));
        assert!(!Rebalance::Quarterly.due(d(2024, 1, 31), d(2024, 2, 1)));
        assert!(Rebalance::Quarterly.due(d(2024, 3, 29), d(2024, 4, 1)));
        assert!(!Rebalance::None.due(d(2024, 3, 29), d(2024, 4, 1)));
    }

    #[test]
    fn annualising() {
        assert!((annualised_pct(10.0, 252, 252.0).unwrap() - 10.0).abs() < 1e-9);
        assert!((annualised_pct(10.0, 365, 365.0).unwrap() - 10.0).abs() < 1e-9);
        assert!(annualised_pct(-100.0, 10, 252.0).is_none());
        assert_eq!(bars_per_year(["TCS.NS", "AAPL"]), 252.0);
        assert_eq!(bars_per_year(["AAPL", "BTC-USD"]), 365.0);
        assert_eq!(bars_per_year(["EURUSD=X"]), 260.0);
    }

    #[test]
    fn sortino_divides_downside_by_all_periods() {
        // Returns +10%, -10%, +10%, +10%: mean 5%, one -10% shortfall, so the
        // downside deviation is sqrt(0.01 / 4) = 5% per period (not 10%, which
        // is what dividing by the single down period gave). Daily crypto bars:
        // 0.05 * 365 / (0.05 * sqrt(365)) = sqrt(365).
        let curve = [100.0, 110.0, 99.0, 108.9, 119.79];
        let s = sortino(&curve, 365.0).unwrap();
        assert!((s - 365.0_f64.sqrt()).abs() < 1e-9, "got {s}");
        assert_eq!(sortino(&[100.0, 101.0, 102.0, 103.0], 252.0), None);
    }

    #[test]
    fn sharpe_and_sortino_match_technical_rs_at_252() {
        let curve = [100.0, 101.0, 99.5, 102.0, 101.0, 104.0, 103.5];
        let (a, b) = (sharpe(&curve, 252.0), technical::sharpe_ratio(&curve, 0.0));
        assert!((a.unwrap() - b.unwrap()).abs() < 1e-12);
        let (a, b) = (
            sortino(&curve, 252.0),
            technical::sortino_ratio(&curve, 0.0),
        );
        assert!((a.unwrap() - b.unwrap()).abs() < 1e-12);
        // Daily crypto bars annualise over 365 days.
        let ratio = sharpe(&curve, 365.0).unwrap() / sharpe(&curve, 252.0).unwrap();
        assert!((ratio - (365.0f64 / 252.0).sqrt()).abs() < 1e-12);
    }
}
