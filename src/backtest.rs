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
    let buy_hold_return = match closes.first() {
        Some(&first) if first != 0.0 => (closes.last().unwrap() / first - 1.0) * 100.0,
        _ => 0.0,
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
    // Trade-level Sharpe: mean / std-dev of per-trade returns. The equity curve
    // has one point per trade (not per day), so annualizing by √252 would be
    // meaningless — report the raw per-trade ratio instead.
    let sharpe = {
        let rets = technical::daily_returns(&equity);
        if rets.len() < 2 {
            None
        } else {
            let mean = rets.iter().sum::<f64>() / rets.len() as f64;
            let var =
                rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (rets.len() - 1) as f64;
            let sd = var.sqrt();
            if sd == 0.0 { None } else { Some(mean / sd) }
        }
    };

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
    let n = closes
        .len()
        .min(highs.len())
        .min(lows.len())
        .min(volumes.len());
    let mut prev_above = false;
    for i in 10..n {
        if let Some(vwap) =
            technical::vwap(&highs[..=i], &lows[..=i], &closes[..=i], &volumes[..=i])
        {
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
                let pnl_pct = if entry_price != 0.0 {
                    (exit_price / entry_price - 1.0) * 100.0
                } else {
                    0.0
                };
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
        let pnl_pct = if entry_price != 0.0 {
            (exit_price / entry_price - 1.0) * 100.0
        } else {
            0.0
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rising_prices(n: usize) -> Vec<f64> {
        (0..n).map(|i| 100.0 + i as f64 * 0.5).collect()
    }

    fn sine_prices(n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| 100.0 + (i as f64 * 0.1).sin() * 10.0)
            .collect()
    }

    #[test]
    fn test_rsi_strategy_generates_signals() {
        let prices = sine_prices(200);
        let result = run_backtest("rsi", &prices, &prices, &prices, &vec![1000u64; 200]);
        assert!(result.is_some());
    }

    #[test]
    fn test_macd_strategy() {
        let prices = sine_prices(200);
        let result = run_backtest("macd", &prices, &prices, &prices, &vec![1000u64; 200]);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.equity_curve.is_empty());
    }

    #[test]
    fn test_sma_crossover_strategy() {
        let prices = sine_prices(200);
        let result = run_backtest("sma", &prices, &prices, &prices, &vec![1000u64; 200]);
        assert!(result.is_some());
    }

    #[test]
    fn test_bollinger_strategy() {
        let prices = sine_prices(200);
        let result = run_backtest("bb", &prices, &prices, &prices, &vec![1000u64; 200]);
        assert!(result.is_some());
    }

    #[test]
    fn test_mean_reversion_strategy() {
        let prices = sine_prices(200);
        let result = run_backtest("mr", &prices, &prices, &prices, &vec![1000u64; 200]);
        assert!(result.is_some());
    }

    #[test]
    fn test_unknown_strategy_returns_none() {
        let prices = vec![100.0; 50];
        let result = run_backtest("unknown", &prices, &prices, &prices, &vec![1000u64; 50]);
        assert!(result.is_none());
    }

    #[test]
    fn test_backtest_equity_curve_starts_at_100() {
        let prices = rising_prices(200);
        let result = run_backtest("rsi", &prices, &prices, &prices, &vec![1000u64; 200]).unwrap();
        assert_eq!(result.equity_curve[0], 100.0);
    }

    #[test]
    fn test_win_rate_bounds() {
        let prices = sine_prices(200);
        let result = run_backtest("rsi", &prices, &prices, &prices, &vec![1000u64; 200]).unwrap();
        assert!(result.win_rate >= 0.0 && result.win_rate <= 100.0);
    }

    #[test]
    fn test_buy_hold_return_matches() {
        let prices = vec![100.0, 110.0, 120.0, 130.0, 140.0, 150.0];
        // Not enough for any strategy, but buy_hold should still work
        let result = run_backtest("rsi", &prices, &prices, &prices, &[1000u64; 6]);
        if let Some(r) = result {
            assert!((r.buy_hold_return - 50.0).abs() < 0.01);
        }
    }

    #[test]
    fn test_execute_trades_closes_open_position() {
        let closes = vec![100.0, 105.0, 110.0];
        let signals = vec![Signal::Buy, Signal::Hold, Signal::Hold];
        let trades = execute_trades(&closes, &signals);
        assert_eq!(trades.len(), 1);
        assert!((trades[0].exit_price - 110.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_execute_trades_buy_sell_pair() {
        let closes = vec![100.0, 105.0, 110.0, 108.0];
        let signals = vec![Signal::Buy, Signal::Hold, Signal::Sell, Signal::Hold];
        let trades = execute_trades(&closes, &signals);
        assert_eq!(trades.len(), 1);
        assert!((trades[0].entry_price - 100.0).abs() < f64::EPSILON);
        assert!((trades[0].exit_price - 110.0).abs() < f64::EPSILON);
        assert!(trades[0].pnl_pct > 0.0);
    }

    #[test]
    fn test_vwap_strategy() {
        let prices = sine_prices(200);
        let vols: Vec<u64> = (0..200).map(|i| 1000 + i as u64 * 10).collect();
        let result = run_backtest("vwap", &prices, &prices, &prices, &vols);
        assert!(result.is_some());
    }

    #[test]
    fn test_rsi_strategy() {
        let prices = sine_prices(200);
        let result = run_backtest("rsi", &prices, &prices, &prices, &vec![1000u64; 200]);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.win_rate >= 0.0 && r.win_rate <= 100.0);
        assert!(r.equity_curve[0] == 100.0);
    }

    #[test]
    fn test_backtest_result_fields() {
        let prices = sine_prices(200);
        let r = run_backtest("bb", &prices, &prices, &prices, &vec![1000u64; 200]).unwrap();
        assert!(!r.strategy.is_empty());
        assert!(r.max_drawdown >= 0.0);
    }

    #[test]
    fn test_execute_trades_no_signals() {
        let closes = vec![100.0, 102.0, 105.0];
        let signals = vec![Signal::Hold, Signal::Hold, Signal::Hold];
        let trades = execute_trades(&closes, &signals);
        assert!(trades.is_empty());
    }

    #[test]
    fn test_execute_trades_multiple_pairs() {
        let closes = vec![100.0, 105.0, 110.0, 108.0, 112.0, 115.0];
        let signals = vec![
            Signal::Buy,
            Signal::Hold,
            Signal::Sell,
            Signal::Buy,
            Signal::Hold,
            Signal::Hold,
        ];
        let trades = execute_trades(&closes, &signals);
        // First pair closed at index 2, second remains open at end
        assert_eq!(trades.len(), 2);
        assert!(trades[0].pnl_pct > 0.0); // bought at 100, sold at 110
    }

    #[test]
    fn test_buy_hold_rising_prices() {
        let prices = rising_prices(100);
        let r = run_backtest("rsi", &prices, &prices, &prices, &vec![1000u64; 100]).unwrap();
        assert!(r.buy_hold_return > 0.0);
    }
}
