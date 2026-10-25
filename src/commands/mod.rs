//! Command handlers, one module per area. `main.rs` parses arguments and
//! calls the `cmd_*` functions re-exported at the bottom of this file.
//!
//! The imports and helpers here are shared: every submodule starts with
//! `use super::*;`.

use crate::alerts::{AlertCondition, AlertStore};
use crate::api::YahooClient;
use crate::backtest;
use crate::charts;
use crate::display::*;
use crate::i18n::{Msg, t, tf};
use crate::insights;
use crate::intraday;
use crate::longterm;
use crate::market::{self, Market};
use crate::output::{self, CsvRow, Format, round, round_opt};
use crate::portfolio::Portfolio;
use crate::simulator::{SimHistory, SimSession, SimTrade};
use crate::technical;
use crate::views;
use crate::watchlist::Watchlist;
use crate::wealth::WealthHistory;
use anyhow::{Context, Result, bail};
use colored::Colorize;
use serde::Serialize;

/// Bar interval for a look-back period (as `technical` fetches it).
pub(crate) fn period_to_interval(period: &str) -> &str {
    match period {
        "1d" => "5m",
        "5d" | "1w" => "15m",
        "1mo" => "1d",
        "3mo" | "6mo" => "1d",
        "1y" | "2y" => "1wk",
        "5y" | "10y" | "max" => "1mo",
        _ => "1d",
    }
}

/// An AI answer, indented, with the one-line educational disclaimer under
/// it. Every AI section ends this way.
fn print_ai_answer(text: &str) {
    for line in text.lines() {
        println!("  {}", line);
    }
    println!("  {}", crate::ai::DISCLAIMER.dimmed().italic());
}

fn resolve_symbols(symbols: &[String], market: Market) -> Vec<String> {
    symbols
        .iter()
        .map(|s| market::resolve_symbol(s, market))
        .collect()
}

mod alerting;
mod assistant;
mod backtests;
mod bonds;
mod broker_selftest;
mod broker_sync;
mod calendar;
mod commodities;
mod fno;
mod fundamentals;
mod funds;
mod holdings;
mod imports;
mod interfaces;
mod markets;
mod overview;
mod planning;
mod portfolio;
mod practice;
mod risk;
mod setup;
mod sim;
mod strategies;
mod technicals;
mod trade;
mod worker;

#[cfg(test)]
mod output_tests;

pub use alerting::*;
pub use assistant::*;
pub use backtests::*;
pub use bonds::*;
pub use broker_selftest::*;
pub use broker_sync::*;
pub use calendar::*;
pub use commodities::*;
pub use fno::*;
pub use fundamentals::*;
pub use funds::*;
pub use holdings::*;
pub use interfaces::*;
pub use markets::*;
pub use overview::*;
pub use planning::*;
pub use portfolio::*;
pub use practice::*;
pub use risk::*;
pub use setup::*;
pub use sim::*;
pub use strategies::*;
pub use technicals::*;
pub use trade::*;
pub use worker::*;
