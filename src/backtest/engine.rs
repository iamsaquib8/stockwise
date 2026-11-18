//! The simulator behind every backtest.
//!
//! Each symbol gets a sleeve of the capital (its weight). By default a sleeve
//! is either all cash or long a whole number of shares bought with all of its
//! cash; [`Execution`] can size positions differently, trade fractional
//! shares, go short, trail stops, pay interest on idle cash and charge tax on
//! gains. A single-symbol backtest is a portfolio of one. Signals are evaluated at
//! each bar's close and, by default, filled at the next bar's open, so a
//! trade never uses a price it could not have known. Stops and targets are
//! checked against each bar's low and high (a gap through the level fills at
//! the open; when both are touched in one bar the stop is assumed first).
//! Every fill pays slippage and the cost model's charges.

use super::costs::{Charges, CostModel, Side};
use super::rules::Signals;
use super::{Bars, date, day, indicators};
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
    TrailingStop,
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

/// Which way a strategy trades: an entry signal buys (long) or sells short.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Long,
    Short,
}

/// How much of a sleeve one entry commits.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Sizing {
    /// All of the sleeve's cash (the original behaviour).
    #[default]
    AllIn,
    /// This percent of the sleeve's value.
    FixedFraction { fraction_pct: f64 },
    /// Shares such that a move of `atr_mult` × ATR against the position
    /// loses `risk_pct` percent of the sleeve's value. Capped at the cash.
    AtrRisk { risk_pct: f64, atr_mult: f64 },
}

/// Capital-gains tax taken from cash when a round trip closes with a gain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TaxDrag {
    pub short_term_pct: f64,
    pub long_term_pct: f64,
    /// Calendar days held before a gain counts as long-term.
    pub long_term_days: u32,
}

/// Position sizing, direction, trailing stops, financing and tax. The
/// default reproduces the original engine exactly: long, all-in, whole
/// shares, no interest, no tax.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Execution {
    pub direction: Direction,
    pub sizing: Sizing,
    /// Allow fractional quantities (US shares, crypto), down to 1e-6.
    pub fractional: bool,
    /// ATR period for `atr-risk` sizing and `trailing_atr_mult`.
    pub atr_period: usize,
    /// Exit when the price falls this percent from the best price since
    /// entry (rises, for a short).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailing_stop_pct: Option<f64>,
    /// Chandelier exit: best price since entry minus this many ATRs (plus,
    /// for a short).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailing_atr_mult: Option<f64>,
    /// Yearly fee for borrowing shares while short, in percent of their value.
    pub borrow_rate_pct: f64,
    /// Yearly interest on idle cash, in percent, compounded per calendar day.
    pub cash_yield_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax: Option<TaxDrag>,
}

impl Default for Execution {
    fn default() -> Execution {
        Execution {
            direction: Direction::Long,
            sizing: Sizing::AllIn,
            fractional: false,
            atr_period: 14,
            trailing_stop_pct: None,
            trailing_atr_mult: None,
            borrow_rate_pct: 0.0,
            cash_yield_pct: 0.0,
            tax: None,
        }
    }
}

impl Execution {
    pub fn is_default(&self) -> bool {
        *self == Execution::default()
    }

    pub fn is_short(&self) -> bool {
        self.direction == Direction::Short
    }

    /// Whether the simulation needs an ATR series.
    fn uses_atr(&self) -> bool {
        matches!(self.sizing, Sizing::AtrRisk { .. }) || self.trailing_atr_mult.is_some()
    }
}

/// A quantity as JSON: an integer when whole (as before fractional shares
/// existed), otherwise a decimal.
fn serialize_qty<S: serde::Serializer>(q: &f64, s: S) -> Result<S::Ok, S::Error> {
    if q.fract() == 0.0 && q.abs() < 9.0e15 {
        s.serialize_i64(*q as i64)
    } else {
        s.serialize_f64(*q)
    }
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
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
    pub execution: Execution,
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
            execution: Execution::default(),
        }
    }

    /// The same run without trading costs, borrow fees or tax: the "gross"
    /// result printed next to the net one.
    pub fn gross(&self) -> SimConfig {
        SimConfig {
            costs: CostModel::none(),
            execution: Execution {
                borrow_rate_pct: 0.0,
                tax: None,
                ..self.execution.clone()
            },
            ..self.clone()
        }
    }
}

/// A round trip: from flat to flat.
#[derive(Debug, Clone, Serialize)]
pub struct Trade {
    pub symbol: String,
    pub entry_date: String,
    pub exit_date: String,
    /// Shares bought (or sold short) on entry.
    #[serde(serialize_with = "serialize_qty")]
    pub qty: f64,
    /// Average entry price, before slippage.
    pub entry_price: f64,
    /// Price of the closing trade, before slippage.
    pub exit_price: f64,
    pub gross_pnl: f64,
    /// Charges plus slippage, both legs, plus any borrow fee.
    pub costs: f64,
    pub net_pnl: f64,
    /// Net P&L as a percent of the money put in.
    #[serde(rename = "pnl_percent")]
    pub return_pct: f64,
    pub bars_held: usize,
    pub exit_reason: ExitReason,
    /// Capital-gains tax paid when this trade closed (`[tax]`).
    #[serde(skip_serializing_if = "is_zero")]
    pub tax: f64,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct CostTotals {
    #[serde(flatten)]
    pub charges: Charges,
    pub slippage: f64,
    /// Fees for borrowing shares while short.
    #[serde(skip_serializing_if = "is_zero")]
    pub borrow: f64,
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
    /// Interest earned on idle cash (`[cash] yield_pct`).
    pub interest: f64,
    /// Capital-gains tax paid (`[tax]`).
    pub tax: f64,
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
    /// Shares held (long) or owed (short); always positive.
    qty: f64,
    /// Average entry price before slippage — stops and targets key off it.
    avg: f64,
    entry_bar: usize,
    entry_time: i64,
    entry_qty: f64,
    /// Gross profit already realised by partial sells (rebalancing).
    gross: f64,
    costs: f64,
    /// Money put in: shares × raw price, summed over every buy.
    basis: f64,
    /// Best price since entry: the highest high for a long, the lowest low
    /// for a short. Trailing stops hang off it.
    extreme: f64,
    /// Cash received from the short sale. It backs the short, so it earns
    /// no interest. Zero for a long.
    proceeds: f64,
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
    /// Realised losses not yet set off against a taxed gain.
    tax_losses: f64,
}

impl Sleeve {
    /// Market value of the open position: positive long, negative short.
    fn held_value(&self, price: f64, short: bool) -> f64 {
        let q = self.pos.as_ref().map_or(0.0, |p| p.qty);
        if short { -q * price } else { q * price }
    }
}

struct Sim<'a> {
    assets: &'a [Asset<'a>],
    cfg: &'a SimConfig,
    sleeves: Vec<Sleeve>,
    costs: CostTotals,
    skipped: usize,
    /// ATR per asset, when sizing or a trailing stop needs it.
    atr: Vec<Vec<Option<f64>>>,
    interest: f64,
    tax: f64,
}

impl Sim<'_> {
    fn short(&self) -> bool {
        self.cfg.execution.is_short()
    }

    /// ATR known when trading at bar `bar`: the previous close's for a fill
    /// at the open, this bar's for a fill at its close.
    fn known_atr(&self, k: usize, bar: usize, at_close: bool) -> Option<f64> {
        let i = if at_close { bar } else { bar.checked_sub(1)? };
        self.atr.get(k)?.get(i).copied().flatten()
    }

    /// Most money an entry (or top-up) may commit at price `raw`, before
    /// charges come out of it. `None` when sizing can't be worked out yet
    /// (ATR still warming up).
    fn budget(&self, k: usize, bar: usize, raw: f64, at_close: bool) -> Option<f64> {
        let s = &self.sleeves[k];
        let short = self.short();
        let held = s.pos.as_ref().map_or(0.0, |p| p.qty);
        let value = s.cash + s.held_value(raw, short);
        let target = match self.cfg.execution.sizing {
            Sizing::AllIn => f64::INFINITY,
            Sizing::FixedFraction { fraction_pct } => value * fraction_pct / 100.0,
            Sizing::AtrRisk { risk_pct, atr_mult } => {
                let atr = self.known_atr(k, bar, at_close)?;
                if atr <= 0.0 {
                    return None;
                }
                value * risk_pct / 100.0 / (atr_mult * atr) * raw
            }
        };
        // A short is backed by cash worth its full value (no leverage).
        let room = if short {
            s.cash - 2.0 * held * raw
        } else {
            s.cash
        };
        Some(room.min(target - held * raw))
    }

    /// Open a position, or add to one (`top_up`, from a rebalance).
    fn enter(&mut self, k: usize, bar: usize, raw: f64, top_up: bool, at_close: bool) {
        let short = self.short();
        let side = if short { Side::Sell } else { Side::Buy };
        let fractional = self.cfg.execution.fractional;
        let budget = self.budget(k, bar, raw, at_close);
        let model = &self.cfg.costs;
        let px = model.fill_price(side, raw);
        let cost = |q: f64| {
            let t = q * px;
            t + model.charges(side, t).total()
        };
        let mut qty = 0.0;
        if let Some(budget) = budget
            && px > 0.0
        {
            let room = budget - model.charges(side, budget.max(0.0)).total();
            if fractional {
                // Charges need not be linear (caps, flat fees): search for
                // the largest quantity that fits, then round down to 1e-6.
                let (mut lo, mut hi) = (0.0, budget.max(0.0) / px);
                for _ in 0..100 {
                    let mid = (lo + hi) / 2.0;
                    if cost(mid) <= budget {
                        lo = mid
                    } else {
                        hi = mid
                    }
                }
                qty = (lo * 1e6).floor() / 1e6;
                while qty > 0.0 && cost(qty) > budget {
                    qty = ((qty - 1e-6) * 1e6).round() / 1e6;
                }
            } else {
                qty = if room > 0.0 { (room / px).floor() } else { 0.0 };
                while cost(qty + 1.0) <= budget {
                    qty += 1.0;
                }
                while qty > 0.0 && cost(qty) > budget {
                    qty -= 1.0;
                }
            }
        }
        if qty <= 0.0 {
            if !top_up {
                self.skipped += 1;
            }
            return;
        }
        let turnover = qty * px;
        let ch = model.charges(side, turnover);
        let slip = qty * (px - raw).abs();
        let s = &mut self.sleeves[k];
        if short {
            s.cash += turnover - ch.total();
        } else {
            s.cash -= turnover + ch.total();
        }
        s.costs += ch.total() + slip;
        self.costs.charges.add(&ch);
        self.costs.slippage += slip;
        let proceeds = if short { turnover - ch.total() } else { 0.0 };
        match &mut s.pos {
            Some(p) => {
                p.avg = (p.avg * p.qty + raw * qty) / (p.qty + qty);
                p.qty += qty;
                p.costs += ch.total() + slip;
                p.basis += raw * qty;
                p.proceeds += proceeds;
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
                    basis: raw * qty,
                    extreme: raw,
                    proceeds,
                });
            }
        }
    }

    /// Close the position, or part of it (`qty`, from a rebalance).
    fn exit(&mut self, k: usize, bar: usize, raw: f64, qty: Option<f64>, reason: ExitReason) {
        let short = self.short();
        let side = if short { Side::Buy } else { Side::Sell };
        let model = &self.cfg.costs;
        let s = &mut self.sleeves[k];
        let Some(p) = s.pos.as_mut() else {
            return;
        };
        let qty = qty.unwrap_or(p.qty).min(p.qty);
        if qty <= 0.0 {
            return;
        }
        let px = model.fill_price(side, raw);
        let turnover = qty * px;
        let ch = model.charges(side, turnover);
        let slip = qty * (raw - px).abs();
        if short {
            s.cash -= turnover + ch.total();
            p.gross += qty * (p.avg - raw);
            p.proceeds -= p.proceeds * qty / p.qty;
        } else {
            s.cash += turnover - ch.total();
            p.gross += qty * (raw - p.avg);
        }
        s.costs += ch.total() + slip;
        self.costs.charges.add(&ch);
        self.costs.slippage += slip;
        p.costs += ch.total() + slip;
        p.qty -= qty;
        if p.qty > 0.0 {
            return;
        }
        let p = s.pos.take().expect("position checked above");
        let net = p.gross - p.costs;
        let asset = &self.assets[k];
        let exit_time = asset.bars.time[bar];
        let mut tax = 0.0;
        if let Some(t) = self.cfg.execution.tax {
            if net <= 0.0 {
                s.tax_losses -= net;
            } else {
                let taxable = (net - s.tax_losses).max(0.0);
                s.tax_losses = (s.tax_losses - net).max(0.0);
                let long_term = exit_time - p.entry_time >= i64::from(t.long_term_days) * 86_400;
                let rate = if long_term {
                    t.long_term_pct
                } else {
                    t.short_term_pct
                };
                tax = taxable * rate / 100.0;
                s.cash -= tax;
                self.tax += tax;
            }
        }
        s.trades.push(Trade {
            symbol: asset.symbol.to_string(),
            entry_date: date(p.entry_time),
            exit_date: date(exit_time),
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
            tax,
        });
    }

    /// Overnight carry from calendar day `from` to `to`: interest on idle
    /// cash and the borrow fee on a short.
    fn carry(&mut self, from: i64, to: i64) {
        let x = &self.cfg.execution;
        let days = (to - from) as f64;
        if days <= 0.0 || (x.cash_yield_pct == 0.0 && x.borrow_rate_pct == 0.0) {
            return;
        }
        let growth = (1.0 + x.cash_yield_pct / 100.0).powf(days / 365.0) - 1.0;
        let borrow_rate = x.borrow_rate_pct / 100.0 * days / 365.0;
        for s in &mut self.sleeves {
            let idle = s.cash - s.pos.as_ref().map_or(0.0, |p| p.proceeds);
            if idle > 0.0 && growth != 0.0 {
                let i = idle * growth;
                s.cash += i;
                self.interest += i;
            }
            if let (Some(p), Some(close)) = (s.pos.as_mut(), s.last_close)
                && p.proceeds > 0.0
                && borrow_rate > 0.0
            {
                let fee = p.qty * close * borrow_rate;
                s.cash -= fee;
                s.costs += fee;
                p.costs += fee;
                self.costs.borrow += fee;
            }
        }
    }

    /// Orders queued at yesterday's close fill at today's open.
    fn on_open(&mut self, k: usize, bar: usize) {
        let open = self.assets[k].bars.open[bar];
        let s = &mut self.sleeves[k];
        let (entry, exit) = (s.pending_entry, s.pending_exit);
        s.pending_entry = false;
        s.pending_exit = false;
        if exit {
            self.exit(k, bar, open, None, ExitReason::Signal);
        }
        if entry && self.sleeves[k].pos.is_none() {
            self.enter(k, bar, open, false, false);
        }
    }

    /// The tightest protective stop for the open position: the fixed stop
    /// and the trailing stops, whichever would exit first.
    fn stop_level(&self, k: usize, bar: usize, p: &Position) -> Option<(f64, ExitReason)> {
        let x = &self.cfg.execution;
        let short = x.is_short();
        let mut best: Option<(f64, ExitReason)> = None;
        let mut consider = |level: f64, reason: ExitReason| {
            let tighter = best.is_none_or(|(b, _)| if short { level < b } else { level > b });
            if tighter {
                best = Some((level, reason));
            }
        };
        if let Some(sl) = self.cfg.stop_loss_pct {
            let f = if short {
                1.0 + sl / 100.0
            } else {
                1.0 - sl / 100.0
            };
            consider(p.avg * f, ExitReason::StopLoss);
        }
        if let Some(t) = x.trailing_stop_pct {
            let f = if short {
                1.0 + t / 100.0
            } else {
                1.0 - t / 100.0
            };
            consider(p.extreme * f, ExitReason::TrailingStop);
        }
        if let Some(m) = x.trailing_atr_mult
            && let Some(atr) = self.known_atr(k, bar, false)
        {
            let level = if short {
                p.extreme + m * atr
            } else {
                p.extreme - m * atr
            };
            consider(level, ExitReason::TrailingStop);
        }
        best
    }

    /// Stop and target exits. With `at_open`, only gaps through a level at
    /// the open (checked before a rebalance trades at that open); otherwise
    /// also touches during the bar.
    fn check_stops(&mut self, k: usize, bar: usize, at_open: bool) {
        let b = self.assets[k].bars;
        let (o, h, l) = (b.open[bar], b.high[bar], b.low[bar]);
        let short = self.short();
        let Some(p) = &self.sleeves[k].pos else {
            return;
        };
        // A position opened at this bar's open can't gap through its own level.
        let gap = p.entry_bar < bar;
        // For a long the adverse side is down; for a short, up.
        let (adverse_open, adverse_touch) = if short { (-o, -h) } else { (o, l) };
        let (favour_open, favour_touch) = if short { (-o, -l) } else { (o, h) };
        let sign = if short { -1.0 } else { 1.0 };
        let stop = self.stop_level(k, bar, p);
        let target = self
            .cfg
            .take_profit_pct
            .map(|tp| p.avg * (1.0 + sign * tp / 100.0));
        if let Some((stop, reason)) = stop {
            if gap && adverse_open <= sign * stop {
                return self.exit(k, bar, o, None, reason);
            }
            if !at_open && adverse_touch <= sign * stop {
                return self.exit(k, bar, stop, None, reason);
            }
        }
        if let Some(target) = target {
            if gap && favour_open >= sign * target {
                return self.exit(k, bar, o, None, ExitReason::TakeProfit);
            }
            if !at_open && favour_touch >= sign * target {
                return self.exit(k, bar, target, None, ExitReason::TakeProfit);
            }
        }
        if !at_open && let Some(p) = self.sleeves[k].pos.as_mut() {
            p.extreme = if short {
                p.extreme.min(l)
            } else {
                p.extreme.max(h)
            };
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
                    self.exit(k, bar, close, None, ExitReason::Signal);
                } else if !holding && entry && !exit {
                    self.enter(k, bar, close, false, true);
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

    /// Bring every sleeve back to its target weight at today's open. Long
    /// positions only: a short sleeve is never rebalanced.
    fn rebalance(&mut self, today: &[Option<usize>]) {
        let n = self.assets.len();
        let fractional = self.cfg.execution.fractional;
        let price = |sim: &Self, k: usize| {
            today[k]
                .map(|b| sim.assets[k].bars.open[b])
                .or(sim.sleeves[k].last_close)
                .unwrap_or(0.0)
        };
        let held = |sim: &Self, k: usize| sim.sleeves[k].pos.as_ref().map_or(0.0, |p| p.qty);
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
                let qty = if fractional {
                    (excess / px * 1e6).floor() / 1e6
                } else {
                    (excess / px).floor()
                };
                self.exit(k, bar, px, Some(qty), ExitReason::Rebalance);
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
                self.enter(k, bar, price(self, k), true, false);
            }
        }
    }
}

/// Run `assets` through the strategy signals they carry.
pub fn simulate(assets: &[Asset], cfg: &SimConfig) -> SimResult {
    let n = assets.len();
    let x = &cfg.execution;
    let short = x.is_short();
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
            tax_losses: 0.0,
        })
        .collect();
    let atr = if x.uses_atr() {
        assets
            .iter()
            .map(|a| indicators::atr(&a.bars.high, &a.bars.low, &a.bars.close, x.atr_period))
            .collect()
    } else {
        Vec::new()
    };
    let mut sim = Sim {
        assets,
        cfg,
        sleeves,
        costs: CostTotals::default(),
        skipped: 0,
        atr,
        interest: 0.0,
        tax: 0.0,
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
        if di > 0 {
            sim.carry(days[di - 1], d);
        }
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
        if di > 0 && cfg.rebalance.due(days[di - 1], d) && n > 1 && !short {
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
                // This symbol has no more bars: close the position at its
                // last close, so borrow fees, tax and the exit date all stop
                // there, even if other symbols trade on.
                if s.cursor == assets[k].bars.len() && s.pos.is_some() {
                    sim.exit(
                        k,
                        bar,
                        assets[k].bars.close[bar],
                        None,
                        ExitReason::EndOfData,
                    );
                }
            }
        }
        let (mut eq, mut inv) = (0.0, 0.0);
        for s in &mut sim.sleeves {
            let held = s.held_value(s.last_close.unwrap_or(0.0), short);
            eq += s.cash + held;
            inv += held.abs();
            if s.last_close.is_some() {
                s.days_listed += 1;
                s.days_held += (held != 0.0) as usize;
            }
        }
        times.push(d * 86_400);
        equity.push(eq);
        invested.push(inv);
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
    costs.total = costs.charges.total() + costs.slippage + costs.borrow;

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
        interest: sim.interest,
        tax: sim.tax,
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
        assert_eq!(t.qty, 90.0); // floor(1000 / 11)
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
        assert_eq!(t.qty, 99.0);
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

    // ── v2 execution options (#394) ──

    fn with(x: Execution, capital: f64) -> SimConfig {
        SimConfig {
            execution: x,
            ..SimConfig::single(capital, Fill::NextOpen, CostModel::none())
        }
    }

    /// Flat bars at `price` with a fixed high-low range of 2, so every
    /// ATR is 2.
    fn flat(n: usize, price: f64) -> Bars {
        let mut b = Bars::from_closes(&vec![price; n]);
        for i in 0..n {
            b.high[i] = price + 1.0;
            b.low[i] = price - 1.0;
        }
        b
    }

    #[test]
    fn fixed_fraction_sizing_commits_part_of_the_sleeve() {
        let b = bars(&[10.0; 4], &[10.0; 4]);
        let x = Execution {
            sizing: Sizing::FixedFraction { fraction_pct: 50.0 },
            ..Execution::default()
        };
        let r = run(&b, &signals(4, &[0], &[]), &with(x, 1_000.0));
        assert_eq!(r.trades[0].qty, 50.0);
        // Half the money stayed in cash all along.
        assert!(r.invested[1..3].iter().all(|v| (*v - 500.0).abs() < 1e-9));
    }

    #[test]
    fn atr_risk_sizing_risks_the_set_share_of_capital() {
        let b = flat(30, 100.0);
        let x = Execution {
            sizing: Sizing::AtrRisk {
                risk_pct: 1.0,
                atr_mult: 2.5,
            },
            ..Execution::default()
        };
        let r = run(&b, &signals(30, &[20], &[]), &with(x.clone(), 100_000.0));
        // 1% of 1,00,000 = 1,000 at risk; 2.5 × ATR 2 = 5 a share → 200.
        assert_eq!(r.trades[0].qty, 200.0);
        // Before the ATR has 14 bars there is no size, so no entry.
        let r = run(&b, &signals(30, &[3], &[]), &with(x, 100_000.0));
        assert!(r.trades.is_empty());
        assert_eq!(r.skipped_entries, 1);
    }

    #[test]
    fn fractional_shares_use_all_the_cash() {
        let b = bars(&[300.0; 3], &[300.0; 3]);
        let x = Execution {
            fractional: true,
            ..Execution::default()
        };
        let r = run(&b, &signals(3, &[0], &[]), &with(x.clone(), 1_000.0));
        assert!((r.trades[0].qty - 3.333_333).abs() < 1e-9);
        // With charges the quantity still leaves the cash non-negative.
        let cfg = SimConfig {
            costs: CostModel::us(),
            ..with(x, 1_000.0)
        };
        let r = run(&b, &signals(3, &[0], &[]), &cfg);
        assert!(r.trades[0].qty > 3.3 && r.trades[0].qty < 3.333_334);
        assert!(r.equity.iter().all(|e| *e > 0.0));
        // Whole shares by default: 3.
        let r = run(
            &b,
            &signals(3, &[0], &[]),
            &with(Execution::default(), 1_000.0),
        );
        assert_eq!(r.trades[0].qty, 3.0);
    }

    #[test]
    fn percent_trailing_stop_follows_the_high() {
        // Bought at 100 (bar 1 open). Bar 2 reaches 120; bar 3 falls to 105,
        // through the trailing level 120 × 0.9 = 108.
        let mut b = bars(
            &[100.0, 100.0, 110.0, 115.0, 100.0],
            &[100.0, 105.0, 118.0, 110.0, 100.0],
        );
        b.high[2] = 120.0;
        b.low[3] = 105.0;
        let x = Execution {
            trailing_stop_pct: Some(10.0),
            ..Execution::default()
        };
        let r = run(&b, &signals(5, &[0], &[]), &with(x.clone(), 10_000.0));
        let t = &r.trades[0];
        assert_eq!(t.exit_reason, ExitReason::TrailingStop);
        assert!((t.exit_price - 108.0).abs() < 1e-9);
        assert_eq!(t.exit_date, date(b.time[3]));

        // The bar that sets a new high can't also be stopped by it: the
        // level only moves after the bar. Bar 2 here spans 95..120; the
        // level during it is still 105 (bar 1's high) × 0.9 = 94.5. Bar 3
        // stays above 108, and bar 4 gaps below it: filled at the open.
        let mut b2 = b.clone();
        b2.low[2] = 95.0;
        b2.low[3] = 109.0;
        let r = run(&b2, &signals(5, &[0], &[]), &with(x, 10_000.0));
        let t = &r.trades[0];
        assert_eq!(t.exit_reason, ExitReason::TrailingStop);
        assert_eq!(t.exit_date, date(b.time[4]));
        assert!((t.exit_price - 100.0).abs() < 1e-9);
    }

    #[test]
    fn atr_trailing_stop_hangs_below_the_high() {
        // ATR 2 throughout; 3 × ATR below the best high.
        let mut b = flat(25, 100.0);
        b.high[18] = 110.0;
        b.close[18] = 109.0;
        b.open[19] = 108.0;
        b.high[19] = 108.0;
        b.close[19] = 105.0;
        // The level is 110 − 3 × the ATR known at bar 18's close (≈ 102.1).
        b.low[19] = 100.0;
        let x = Execution {
            trailing_atr_mult: Some(3.0),
            ..Execution::default()
        };
        let r = run(&b, &signals(25, &[15], &[]), &with(x, 10_000.0));
        let t = &r.trades[0];
        assert_eq!(t.exit_reason, ExitReason::TrailingStop);
        let atr = indicators::atr(&b.high, &b.low, &b.close, 14)[18].unwrap();
        assert!((t.exit_price - (110.0 - 3.0 * atr)).abs() < 1e-9);
        assert_eq!(t.exit_date, date(b.time[19]));
    }

    #[test]
    fn short_profits_from_a_fall_and_pays_borrow() {
        // Short 100 shares at 100 (bar 1 open), cover at 90 (bar 4 open).
        let b = bars(
            &[100.0, 100.0, 97.0, 93.0, 90.0],
            &[100.0, 100.0, 95.0, 92.0, 90.0],
        );
        let x = Execution {
            direction: Direction::Short,
            borrow_rate_pct: 36.5,
            ..Execution::default()
        };
        let r = run(&b, &signals(5, &[0], &[3]), &with(x, 10_000.0));
        let t = &r.trades[0];
        assert_eq!(t.qty, 100.0);
        assert!((t.gross_pnl - 1_000.0).abs() < 1e-9);
        // 0.1% a day of the previous close's value: 10 + 9.5 + 9.2.
        let borrow = 10.0 + 9.5 + 9.2;
        assert!((r.costs.borrow - borrow).abs() < 1e-9);
        assert!((t.costs - borrow).abs() < 1e-9);
        assert!((t.net_pnl - (1_000.0 - borrow)).abs() < 1e-9);
        assert!((r.equity[4] - (10_000.0 + 1_000.0 - borrow)).abs() < 1e-9);
        // Equity marks the short to market: at bar 2's close (95) it is up 500
        // less one day's borrow.
        assert!((r.equity[2] - (10_500.0 - 10.0)).abs() < 1e-9);
        assert!((r.metrics().costs - borrow).abs() < 1e-9);
    }

    #[test]
    fn short_stop_is_above_the_entry() {
        let mut b = bars(&[100.0, 100.0, 101.0, 101.0], &[100.0, 100.0, 102.0, 101.0]);
        b.high[2] = 110.0;
        let x = Execution {
            direction: Direction::Short,
            ..Execution::default()
        };
        let cfg = SimConfig {
            stop_loss_pct: Some(5.0),
            take_profit_pct: Some(50.0),
            ..with(x, 10_000.0)
        };
        let r = run(&b, &signals(4, &[0], &[]), &cfg);
        let t = &r.trades[0];
        assert_eq!(t.exit_reason, ExitReason::StopLoss);
        assert!((t.exit_price - 105.0).abs() < 1e-9);
        assert!((t.net_pnl + 500.0).abs() < 1e-9);
    }

    #[test]
    fn idle_cash_earns_the_sweep_yield() {
        // 366 daily bars = 365 calendar days, never invested.
        let b = Bars::from_closes(&[100.0; 366]);
        let x = Execution {
            cash_yield_pct: 10.0,
            ..Execution::default()
        };
        let r = run(&b, &signals(366, &[], &[]), &with(x.clone(), 10_000.0));
        assert!((r.equity[365] - 11_000.0).abs() < 1e-6);
        assert!((r.interest - 1_000.0).abs() < 1e-6);
        // Fully invested in a flat stock from day 1: the first night's
        // interest is all that stays in cash, and it keeps compounding.
        let r = run(&b, &signals(366, &[0], &[]), &with(x, 10_000.0));
        let one_day = 10_000.0 * (1.1f64.powf(1.0 / 365.0) - 1.0);
        assert!((r.interest - one_day * 1.1f64.powf(364.0 / 365.0)).abs() < 1e-9);
    }

    #[test]
    fn tax_is_taken_from_gains_after_losses() {
        // Trade 1 buys at 100 and sells at 90; trade 2 buys at 90 and sells
        // at 120. Only the gain left after the first loss is taxed.
        let b = bars(
            &[100.0, 100.0, 90.0, 90.0, 90.0, 120.0, 120.0],
            &[100.0, 100.0, 90.0, 90.0, 90.0, 120.0, 120.0],
        );
        let s = signals(7, &[0, 3], &[1, 4]);
        let x = Execution {
            tax: Some(TaxDrag {
                short_term_pct: 20.0,
                long_term_pct: 12.5,
                long_term_days: 365,
            }),
            ..Execution::default()
        };
        let r = run(&b, &s, &with(x.clone(), 10_000.0));
        let (t1, t2) = (&r.trades[0], &r.trades[1]);
        assert!(t1.net_pnl < 0.0 && t1.tax == 0.0);
        let want = (t2.net_pnl + t1.net_pnl) * 0.20;
        assert!((t2.tax - want).abs() < 1e-9, "{} vs {want}", t2.tax);
        assert!((r.tax - want).abs() < 1e-9);
        let pre_tax = run(&b, &s, &with(Execution::default(), 10_000.0));
        let drag = pre_tax.equity[6] - r.equity[6];
        assert!((drag - want).abs() < 1e-9);
        // The gross run ignores tax.
        assert!(with(x, 10_000.0).gross().execution.tax.is_none());
    }

    #[test]
    fn long_held_gains_pay_the_long_term_rate() {
        let mut closes = vec![100.0; 400];
        closes[399] = 110.0;
        let b = Bars::from_closes(&closes);
        let x = Execution {
            tax: Some(TaxDrag {
                short_term_pct: 20.0,
                long_term_pct: 12.5,
                long_term_days: 365,
            }),
            ..Execution::default()
        };
        let r = run(&b, &signals(400, &[0], &[]), &with(x, 10_000.0));
        let t = &r.trades[0];
        assert!((t.tax - t.net_pnl * 0.125).abs() < 1e-9);
    }

    #[test]
    fn a_symbol_that_ends_early_stops_paying_borrow_there() {
        // A has three bars, B eight. A's short is closed at A's last close;
        // borrow is charged for the two nights A traded, not after.
        let a = bars(&[100.0; 3], &[100.0; 3]);
        let b = bars(&[50.0; 8], &[50.0; 8]);
        let (sa, sb) = (signals(3, &[0], &[]), signals(8, &[], &[]));
        let assets = [
            Asset {
                symbol: "A",
                bars: &a,
                signals: &sa,
            },
            Asset {
                symbol: "B",
                bars: &b,
                signals: &sb,
            },
        ];
        let x = Execution {
            direction: Direction::Short,
            borrow_rate_pct: 36.5,
            ..Execution::default()
        };
        let r = simulate(&assets, &with(x, 20_000.0));
        let t = &r.trades[0];
        assert_eq!(t.exit_date, date(a.time[2]));
        // 100 shares × 100 × 0.1% for one night (bar 1 → bar 2).
        assert!((r.costs.borrow - 10.0).abs() < 1e-9, "{}", r.costs.borrow);
        assert!((t.costs - 10.0).abs() < 1e-9);
        assert!(r.invested[3..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn whole_quantities_serialise_as_integers() {
        let v = |q: f64| {
            let mut s = serde_json::Serializer::new(Vec::new());
            serialize_qty(&q, &mut s).unwrap();
            String::from_utf8(s.into_inner()).unwrap()
        };
        assert_eq!(v(583.0), "583");
        assert_eq!(v(3.25), "3.25");
    }
}
