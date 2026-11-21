//! `portfolio add/sell --charges N|auto`: the charges recorded on a lot or a
//! sale. `auto` estimates an Indian delivery (CNC) trade's statutory charges
//! with the backtest cost model ([`crate::backtest::costs`]), the same rates
//! the `trade` order preview uses: STT 0.1% on both sides, stamp duty
//! 0.015% on the buy, the NSE transaction charge in force on the trade date,
//! the SEBI fee, 18% GST on those, and a DP charge on the sell. Brokerage
//! depends on the broker and is left out (add it with `--charges N`). The
//! contract note is the final word.

use crate::backtest::costs::{self, Brokerage, Charges, CostModel, Side};
use chrono::NaiveDate;

/// What `--charges` asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChargesArg {
    /// A total typed in, in the trade's currency.
    Amount(f64),
    /// Estimate an Indian delivery trade's statutory charges.
    Auto,
}

/// clap value parser: a non-negative number or `auto`.
pub fn parse(s: &str) -> Result<ChargesArg, String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("auto") {
        return Ok(ChargesArg::Auto);
    }
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(ChargesArg::Amount(v)),
        _ => Err(format!(
            "expected an amount (e.g. 23.60) or `auto`, got `{s}`"
        )),
    }
}

/// The statutory charges of an Indian delivery trade of `turnover` rupees
/// on `date` (no brokerage).
pub fn estimate_delivery(sell: bool, turnover: f64, date: NaiveDate) -> Charges {
    let mut m = CostModel::in_delivery();
    m.brokerage = Brokerage {
        flat: Some(0.0),
        pct: None,
    };
    m.exchange_pct = costs::nse_exchange_pct_on(date);
    m.charges(if sell { Side::Sell } else { Side::Buy }, turnover)
}

/// "STT ₹100.00 · stamp ₹15.00 · exchange ₹3.07 · SEBI ₹0.10 · GST ₹0.57".
pub fn breakdown(c: &Charges) -> String {
    let rupees = |v: f64| crate::display::format_price(v, Some("INR"));
    let mut parts = vec![format!("STT {}", rupees(c.stt))];
    if c.stamp > 0.0 {
        parts.push(format!("stamp {}", rupees(c.stamp)));
    }
    parts.push(format!("exchange {}", rupees(c.exchange)));
    parts.push(format!("SEBI {}", rupees(c.sebi)));
    parts.push(format!("GST {}", rupees(c.gst)));
    if c.dp > 0.0 {
        parts.push(format!("DP {}", rupees(c.dp)));
    }
    parts.join(" · ")
}

/// Whether `auto` applies to `symbol`: an NSE or BSE share or ETF.
pub fn auto_applies(symbol: &str) -> bool {
    let s = symbol.to_ascii_uppercase();
    (s.ends_with(".NS") || s.ends_with(".BO")) && !crate::offmarket::is_off_market(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn parses_amounts_and_auto() {
        assert_eq!(parse("auto"), Ok(ChargesArg::Auto));
        assert_eq!(parse("AUTO"), Ok(ChargesArg::Auto));
        assert_eq!(parse("23.6"), Ok(ChargesArg::Amount(23.6)));
        assert!(parse("-1").is_err());
        assert!(parse("lots").is_err());
    }

    /// ₹1,00,000 delivery buy on 18 Nov 2026 (exchange 0.00307%):
    ///   STT 0.1% = ₹100, stamp 0.015% = ₹15, exchange ₹3.07, SEBI ₹0.10,
    ///   GST 18% × 3.17 = ₹0.5706; total ₹118.7406.
    #[test]
    fn a_one_lakh_delivery_buy() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let c = estimate_delivery(false, 100_000.0, d);
        assert!(close(c.brokerage, 0.0));
        assert!(close(c.stt, 100.0));
        assert!(close(c.stamp, 15.0));
        assert!(close(c.exchange, 3.07));
        assert!(close(c.sebi, 0.10));
        assert!(close(c.gst, 0.5706));
        assert!(close(c.dp, 0.0));
        assert!(close(c.total(), 118.7406));
    }

    /// The same as a sell: no stamp duty, a DP charge instead.
    #[test]
    fn a_one_lakh_delivery_sell() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let c = estimate_delivery(true, 100_000.0, d);
        assert!(close(c.stt, 100.0));
        assert!(close(c.stamp, 0.0));
        assert!(c.dp > 0.0);
        assert!(close(c.total(), 100.0 + 3.07 + 0.10 + 0.5706 + c.dp));
        assert!(breakdown(&c).contains("DP"));
        assert!(!breakdown(&c).contains("stamp"));
    }

    #[test]
    fn auto_is_for_indian_listings_only() {
        assert!(auto_applies("TCS.NS"));
        assert!(auto_applies("500325.BO"));
        assert!(!auto_applies("AAPL"));
        assert!(!auto_applies("MF:120503"));
    }
}
