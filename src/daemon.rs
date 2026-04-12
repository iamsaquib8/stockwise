use anyhow::Result;
use chrono::{Local, NaiveTime, Datelike, Weekday};

/// Market session phases
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    PreMarket,    // 9:00 – 9:14
    Opening,      // 9:15 – 9:30
    Active,       // 9:30 – 14:00
    WindDown,     // 14:00 – 15:15
    SquareOff,    // 15:15 – 15:20
    PostMarket,   // 15:20 – 16:00
    Closed,       // outside market hours
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::PreMarket => write!(f, "Pre-Market"),
            Phase::Opening => write!(f, "Opening"),
            Phase::Active => write!(f, "Active Trading"),
            Phase::WindDown => write!(f, "Wind-Down"),
            Phase::SquareOff => write!(f, "Square-Off"),
            Phase::PostMarket => write!(f, "Post-Market"),
            Phase::Closed => write!(f, "Closed"),
        }
    }
}

pub fn current_phase_india() -> Phase {
    let now = Local::now();
    let day = now.weekday();
    if day == Weekday::Sat || day == Weekday::Sun {
        return Phase::Closed;
    }
    let time = now.time();
    let t = |h: u32, m: u32| NaiveTime::from_hms_opt(h, m, 0).unwrap();

    if time < t(9, 0) || time >= t(16, 0) {
        Phase::Closed
    } else if time < t(9, 15) {
        Phase::PreMarket
    } else if time < t(9, 30) {
        Phase::Opening
    } else if time < t(14, 0) {
        Phase::Active
    } else if time < t(15, 15) {
        Phase::WindDown
    } else if time < t(15, 20) {
        Phase::SquareOff
    } else {
        Phase::PostMarket
    }
}

pub fn is_market_open_india() -> bool {
    !matches!(current_phase_india(), Phase::Closed)
}

/// Live position tracked by the daemon
#[derive(Debug, Clone)]
pub struct LivePosition {
    pub symbol: String,
    pub direction: String,
    pub entry_price: f64,
    pub current_price: f64,
    pub target1: f64,
    pub target2: f64,
    pub stop_loss: f64,
    pub trailing_stop: f64,
    pub qty: u32,
    pub t1_booked: bool,       // has T1 been partially booked?
    pub remaining_qty: u32,     // qty left after partial booking
    pub pnl: f64,
    pub pnl_pct: f64,
    pub status: PositionStatus,
    pub strategies: Vec<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PositionStatus {
    Open,
    T1Hit,       // partial booked
    T2Hit,       // fully closed at target
    StopHit,     // stopped out
    SquaredOff,  // forced close at EOD
}

impl std::fmt::Display for PositionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PositionStatus::Open => write!(f, "OPEN"),
            PositionStatus::T1Hit => write!(f, "T1 HIT"),
            PositionStatus::T2Hit => write!(f, "TARGET"),
            PositionStatus::StopHit => write!(f, "STOPPED"),
            PositionStatus::SquaredOff => write!(f, "SQUARED OFF"),
        }
    }
}

/// Risk manager state
#[derive(Debug)]
pub struct RiskState {
    pub capital: f64,
    pub daily_loss_limit_pct: f64,
    pub max_positions: usize,
    pub max_position_pct: f64,
    pub realized_pnl: f64,
    pub killed: bool, // kill switch triggered
}

impl RiskState {
    pub fn new(capital: f64) -> Self {
        Self {
            capital,
            daily_loss_limit_pct: 3.0,
            max_positions: 5,
            max_position_pct: 30.0,
            realized_pnl: 0.0,
            killed: false,
        }
    }

    pub fn check_daily_limit(&mut self, unrealized_pnl: f64) -> bool {
        let total = self.realized_pnl + unrealized_pnl;
        let limit = self.capital * (self.daily_loss_limit_pct / 100.0);
        if total < -limit {
            self.killed = true;
            true // limit breached
        } else {
            false
        }
    }

    pub fn can_enter(&self, positions_count: usize) -> bool {
        !self.killed && positions_count < self.max_positions
    }

    pub fn record_exit(&mut self, pnl: f64) {
        self.realized_pnl += pnl;
    }
}

/// Update a position with new price, handle T1/stop/trailing
pub fn update_position(pos: &mut LivePosition, new_price: f64, phase: Phase) {
    pos.current_price = new_price;
    let is_long = pos.direction == "BUY";

    // Calculate P&L
    if is_long {
        pos.pnl = (new_price - pos.entry_price) * pos.remaining_qty as f64;
        pos.pnl_pct = ((new_price / pos.entry_price) - 1.0) * 100.0;
    } else {
        pos.pnl = (pos.entry_price - new_price) * pos.remaining_qty as f64;
        pos.pnl_pct = ((pos.entry_price / new_price) - 1.0) * 100.0;
    }

    if pos.status == PositionStatus::T2Hit || pos.status == PositionStatus::StopHit || pos.status == PositionStatus::SquaredOff {
        return; // already closed
    }

    // Check stop loss
    let stopped = if is_long { new_price <= pos.stop_loss } else { new_price >= pos.stop_loss };
    if stopped {
        pos.status = PositionStatus::StopHit;
        return;
    }

    // Check trailing stop
    let trail_stopped = if is_long { new_price <= pos.trailing_stop } else { new_price >= pos.trailing_stop };
    if trail_stopped && pos.t1_booked {
        pos.status = PositionStatus::StopHit; // trailing stop after T1
        return;
    }

    // Check T2
    let t2_hit = if is_long { new_price >= pos.target2 } else { new_price <= pos.target2 };
    if t2_hit {
        pos.status = PositionStatus::T2Hit;
        return;
    }

    // Check T1 (partial book)
    let t1_hit = if is_long { new_price >= pos.target1 } else { new_price <= pos.target1 };
    if t1_hit && !pos.t1_booked {
        pos.t1_booked = true;
        let book_qty = pos.qty / 2;
        pos.remaining_qty = pos.qty - book_qty;
        // Move stop to breakeven after T1
        pos.stop_loss = pos.entry_price;
        pos.status = PositionStatus::T1Hit;
    }

    // Update trailing stop (only moves in favorable direction)
    if is_long {
        let atr_trail = new_price * 0.015; // ~1.5% trail
        let new_trail = new_price - atr_trail;
        if new_trail > pos.trailing_stop {
            pos.trailing_stop = new_trail;
        }
    } else {
        let atr_trail = new_price * 0.015;
        let new_trail = new_price + atr_trail;
        if new_trail < pos.trailing_stop {
            pos.trailing_stop = new_trail;
        }
    }

    // Wind-down: tighten stops
    if phase == Phase::WindDown {
        if is_long {
            let tight = new_price * 0.995; // 0.5% stop in wind-down
            if tight > pos.stop_loss { pos.stop_loss = tight; }
        }
    }

    // Square-off phase: force close
    if phase == Phase::SquareOff {
        pos.status = PositionStatus::SquaredOff;
    }
}

/// Convert intraday trade plans to live positions
pub fn plans_to_positions(plans: &[crate::intraday::TradePlan]) -> Vec<LivePosition> {
    plans.iter().map(|p| LivePosition {
        symbol: p.signal.symbol.clone(),
        direction: p.signal.direction.to_string(),
        entry_price: p.entry,
        current_price: p.entry,
        target1: p.target1,
        target2: p.target2,
        stop_loss: p.stop_loss,
        trailing_stop: p.trailing_stop,
        qty: p.qty,
        t1_booked: false,
        remaining_qty: p.qty,
        pnl: 0.0,
        pnl_pct: 0.0,
        status: PositionStatus::Open,
        strategies: p.signal.strategies.iter().map(|s| s.name.to_string()).collect(),
        score: p.signal.score,
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_position(entry: f64, t1: f64, t2: f64, stop: f64) -> LivePosition {
        LivePosition {
            symbol: "TEST.NS".into(), direction: "BUY".into(),
            entry_price: entry, current_price: entry, target1: t1, target2: t2,
            stop_loss: stop, trailing_stop: entry - 5.0, qty: 10, t1_booked: false,
            remaining_qty: 10, pnl: 0.0, pnl_pct: 0.0, status: PositionStatus::Open,
            strategies: vec!["RSI".into()], score: 75.0,
        }
    }

    #[test]
    fn test_stop_loss_hit() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 96.0, Phase::Active);
        assert_eq!(pos.status, PositionStatus::StopHit);
    }

    #[test]
    fn test_target1_partial_book() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 103.5, Phase::Active);
        assert_eq!(pos.status, PositionStatus::T1Hit);
        assert!(pos.t1_booked);
        assert_eq!(pos.remaining_qty, 5); // 50% booked
        assert_eq!(pos.stop_loss, 100.0); // moved to breakeven
    }

    #[test]
    fn test_target2_full_close() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 107.0, Phase::Active);
        assert_eq!(pos.status, PositionStatus::T2Hit);
    }

    #[test]
    fn test_trailing_stop_moves_up() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        let initial_trail = pos.trailing_stop;
        update_position(&mut pos, 102.0, Phase::Active);
        assert!(pos.trailing_stop > initial_trail, "Trailing stop should move up");
    }

    #[test]
    fn test_trailing_stop_doesnt_move_down() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 102.0, Phase::Active);
        let trail_after_up = pos.trailing_stop;
        update_position(&mut pos, 101.0, Phase::Active);
        assert_eq!(pos.trailing_stop, trail_after_up, "Trailing stop should not move down");
    }

    #[test]
    fn test_square_off_phase() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 101.0, Phase::SquareOff);
        assert_eq!(pos.status, PositionStatus::SquaredOff);
    }

    #[test]
    fn test_pnl_calculation() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 105.0, Phase::Active);
        assert!((pos.pnl - 50.0).abs() < 0.01); // 10 shares * 5.0
        assert!((pos.pnl_pct - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_risk_state_daily_limit() {
        let mut risk = RiskState::new(25000.0);
        assert!(!risk.killed);
        assert!(risk.can_enter(0));
        // 3% of 25000 = 750
        let breached = risk.check_daily_limit(-800.0);
        assert!(breached);
        assert!(risk.killed);
        assert!(!risk.can_enter(0));
    }

    #[test]
    fn test_risk_position_limit() {
        let risk = RiskState::new(25000.0);
        assert!(risk.can_enter(4));  // 4 < 5 max
        assert!(!risk.can_enter(5)); // 5 = max, can't enter
    }

    #[test]
    fn test_risk_record_exit() {
        let mut risk = RiskState::new(25000.0);
        risk.record_exit(200.0);
        risk.record_exit(-100.0);
        assert!((risk.realized_pnl - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_closed_position_not_updated() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        pos.status = PositionStatus::T2Hit;
        update_position(&mut pos, 50.0, Phase::Active); // should be ignored
        assert_eq!(pos.status, PositionStatus::T2Hit); // still T2Hit, not StopHit
    }

    #[test]
    fn test_wind_down_tightens_stop() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 102.0, Phase::WindDown);
        assert!(pos.stop_loss > 97.0, "Wind-down should tighten stop");
    }
}
