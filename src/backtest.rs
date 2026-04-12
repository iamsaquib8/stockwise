use crate::technical;

#[derive(Debug, Clone)]
pub struct Trade {
    pub entry_idx: usize,
    pub exit_idx: usize,
    pub entry_price: f64,
    pub exit_price: f64,
    pub pnl_pct: f64,
}

#[derive(Debug)]
pub struct BacktestResult {
    pub strategy: String,
    pub trades: Vec<Trade>,
    pub total_return: f64,
    pub win_rate: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub max_drawdown: f64,
    pub sharpe: Option<f64>,
    pub buy_hold_return: f64,
    pub equity_curve: Vec<f64>,
}

pub fn run_backtest(
    strategy: &str,
    closes: &[f64],
    highs: &[f64],
    lows: &[f64],
    volumes: &[u64],
) -> Option<BacktestResult> {
    let signals = match strategy {
        "rsi" => strategy_rsi(closes),
        "macd" => strategy_macd(closes),
        "sma_crossover" | "sma" => strategy_sma_crossover(closes),
        "bollinger" | "bb" => strategy_bollinger(closes),
        "vwap" => strategy_vwap(closes, highs, lows, volumes),
        "mean_reversion" | "mr" => strategy_mean_reversion(closes),
        _ => return None,
    };

    let trades = execute_trades(closes, &signals);
    let buy_hold_return = if !closes.is_empty() {
        (closes.last().unwrap() / closes[0] - 1.0) * 100.0
    } else {
        0.0
    };

    // Build equity curve
    let mut equity = vec![100.0_f64]; // start at 100
    let mut current = 100.0;
    for trade in &trades {
        current *= 1.0 + trade.pnl_pct / 100.0;
        equity.push(current);
    }

    let wins: Vec<&Trade> = trades.iter().filter(|t| t.pnl_pct > 0.0).collect();
    let losses: Vec<&Trade> = trades.iter().filter(|t| t.pnl_pct <= 0.0).collect();
    let win_rate = if trades.is_empty() {
        0.0
    } else {
        wins.len() as f64 / trades.len() as f64 * 100.0
    };
    let avg_win = if wins.is_empty() {
        0.0
    } else {
        wins.iter().map(|t| t.pnl_pct).sum::<f64>() / wins.len() as f64
    };
    let avg_loss = if losses.is_empty() {
        0.0
    } else {
        losses.iter().map(|t| t.pnl_pct).sum::<f64>() / losses.len() as f64
    };
    let total_return = if !equity.is_empty() {
        equity.last().unwrap() - 100.0
    } else {
        0.0
    };
    let max_drawdown = technical::max_drawdown(&equity)
        .map(|(dd, _, _)| dd * 100.0)
        .unwrap_or(0.0);
    let sharpe = technical::sharpe_ratio(&equity, 0.0);

    Some(BacktestResult {
        strategy: strategy.to_string(),
        trades,
        total_return,
        win_rate,
        avg_win,
        avg_loss,
        max_drawdown,
        sharpe,
        buy_hold_return,
        equity_curve: equity,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Signal {
    Buy,
    Sell,
    Hold,
}

fn strategy_rsi(closes: &[f64]) -> Vec<Signal> {
    let mut signals = vec![Signal::Hold; closes.len()];
    for i in 15..closes.len() {
        if let Some(rsi) = technical::rsi(&closes[..=i], 14) {
            if rsi < 30.0 {
                signals[i] = Signal::Buy;
            } else if rsi > 70.0 {
                signals[i] = Signal::Sell;
            }
        }
    }
    signals
}

fn strategy_macd(closes: &[f64]) -> Vec<Signal> {
    let mut signals = vec![Signal::Hold; closes.len()];
    let mut prev_hist = 0.0;
    for i in 35..closes.len() {
        if let Some((_, _, hist)) = technical::macd(&closes[..=i]) {
            if hist > 0.0 && prev_hist <= 0.0 {
                signals[i] = Signal::Buy;
            } else if hist < 0.0 && prev_hist >= 0.0 {
                signals[i] = Signal::Sell;
            }
            prev_hist = hist;
        }
    }
    signals
}

fn strategy_sma_crossover(closes: &[f64]) -> Vec<Signal> {
    let mut signals = vec![Signal::Hold; closes.len()];
    let mut prev_above = false;
    for i in 50..closes.len() {
        let sma20 = technical::sma(&closes[..=i], 20);
        let sma50 = technical::sma(&closes[..=i], 50);
        if let (Some(s20), Some(s50)) = (sma20, sma50) {
            let above = s20 > s50;
            if above && !prev_above {
                signals[i] = Signal::Buy;
            } else if !above && prev_above {
                signals[i] = Signal::Sell;
            }
            prev_above = above;
        }
    }
    signals
}

fn strategy_bollinger(closes: &[f64]) -> Vec<Signal> {
    let mut signals = vec![Signal::Hold; closes.len()];
    for i in 20..closes.len() {
        if let Some((upper, _, lower)) = technical::bollinger_bands(&closes[..=i], 20) {
            if closes[i] < lower {
                signals[i] = Signal::Buy;
            } else if closes[i] > upper {
                signals[i] = Signal::Sell;
            }
        }
    }
    signals
}

fn strategy_vwap(closes: &[f64], highs: &[f64], lows: &[f64], volumes: &[u64]) -> Vec<Signal> {
    let mut signals = vec![Signal::Hold; closes.len()];
    let n = closes.len().min(highs.len()).min(lows.len()).min(volumes.len());
    let mut prev_above = false;
    for i in 10..n {
        if let Some(vwap) = technical::vwap(&highs[..=i], &lows[..=i], &closes[..=i], &volumes[..=i]) {
            let above = closes[i] > vwap;
            if above && !prev_above {
                signals[i] = Signal::Buy;
            } else if !above && prev_above {
                signals[i] = Signal::Sell;
            }
            prev_above = above;
        }
    }
    signals
}

fn strategy_mean_reversion(closes: &[f64]) -> Vec<Signal> {
    let mut signals = vec![Signal::Hold; closes.len()];
    for i in 20..closes.len() {
        if let Some(sma) = technical::sma(&closes[..=i], 20) {
            let deviation = (closes[i] - sma) / sma * 100.0;
            if deviation < -3.0 {
                signals[i] = Signal::Buy;
            } else if deviation > 3.0 {
                signals[i] = Signal::Sell;
            }
        }
    }
    signals
}

fn execute_trades(closes: &[f64], signals: &[Signal]) -> Vec<Trade> {
    let mut trades = Vec::new();
    let mut position: Option<(usize, f64)> = None;

    for (i, signal) in signals.iter().enumerate() {
        match signal {
            Signal::Buy if position.is_none() => {
                position = Some((i, closes[i]));
            }
            Signal::Sell if position.is_some() => {
                let (entry_idx, entry_price) = position.unwrap();
                let exit_price = closes[i];
                let pnl_pct = (exit_price / entry_price - 1.0) * 100.0;
                trades.push(Trade {
                    entry_idx,
                    exit_idx: i,
                    entry_price,
                    exit_price,
                    pnl_pct,
                });
                position = None;
            }
            _ => {}
        }
    }
    // Close open position at end
    if let Some((entry_idx, entry_price)) = position {
        let exit_price = *closes.last().unwrap();
        let pnl_pct = (exit_price / entry_price - 1.0) * 100.0;
        trades.push(Trade {
            entry_idx,
            exit_idx: closes.len() - 1,
            entry_price,
            exit_price,
            pnl_pct,
        });
    }
    trades
}
