//! Trading costs: brokerage, statutory charges, and slippage.
//!
//! The Indian presets follow the charge sheet discount brokers publish for
//! NSE equity (rates as of 2025; override brokerage and slippage from the
//! CLI). Every rate is a percent of the order's turnover unless noted.
//!
//! | Charge                  | Delivery            | Intraday            |
//! |-------------------------|---------------------|---------------------|
//! | Brokerage (per order)   | ₹20 or 0.1%, lower  | ₹20 or 0.03%, lower |
//! | STT                     | 0.1% buy and sell   | 0.025% sell         |
//! | Exchange transaction    | 0.00297%            | 0.00297%            |
//! | SEBI turnover fee       | ₹10 per crore       | ₹10 per crore       |
//! | GST                     | 18% of brokerage + exchange + SEBI fees   ||
//! | Stamp duty              | 0.015% buy          | 0.003% buy          |
//! | DP charge (per sell)    | ₹15.93              | —                   |

use crate::market::AssetClass;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

/// Brokerage per order: a flat amount, a percent of turnover, or the lower
/// of the two (how most Indian discount brokers price it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Brokerage {
    pub flat: Option<f64>,
    #[serde(rename = "percent")]
    pub pct: Option<f64>,
}

impl Brokerage {
    pub fn on(&self, turnover: f64) -> f64 {
        let pct = self.pct.map(|p| turnover * p / 100.0);
        match (self.flat, pct) {
            (Some(f), Some(p)) => f.min(p),
            (Some(f), None) => f,
            (None, Some(p)) => p,
            (None, None) => 0.0,
        }
    }

    /// Parse `20` (flat), `0.03%` (percent), or `20,0.03%` (the lower of both).
    pub fn parse(s: &str) -> Result<Brokerage, String> {
        let mut b = Brokerage::default();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (num, is_pct) = match part.strip_suffix('%') {
                Some(n) => (n.trim(), true),
                None => (part, false),
            };
            let v: f64 = num
                .parse()
                .ok()
                .filter(|v: &f64| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| {
                    format!("`{part}` is not a brokerage: use 20 (flat), 0.03% or 20,0.03%")
                })?;
            let slot = if is_pct { &mut b.pct } else { &mut b.flat };
            if slot.replace(v).is_some() {
                return Err(format!(
                    "`{s}` sets the same brokerage twice: use 20 (flat), 0.03% or 20,0.03%"
                ));
            }
        }
        if b.flat.is_none() && b.pct.is_none() {
            return Err("empty brokerage: use 20 (flat), 0.03% or 20,0.03%".into());
        }
        Ok(b)
    }

    fn describe(&self, csym: &str) -> String {
        match (self.flat, self.pct) {
            (Some(f), Some(p)) => format!("{csym}{f} or {p}%, lower"),
            (Some(f), None) => format!("{csym}{f}/order"),
            (None, Some(p)) => format!("{p}%"),
            (None, None) => "none".into(),
        }
    }
}

/// Charges for one order, by component.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Charges {
    pub brokerage: f64,
    pub stt: f64,
    pub exchange: f64,
    pub sebi: f64,
    pub gst: f64,
    pub stamp: f64,
    pub dp: f64,
}

impl Charges {
    pub fn total(&self) -> f64 {
        self.brokerage + self.stt + self.exchange + self.sebi + self.gst + self.stamp + self.dp
    }

    pub fn add(&mut self, o: &Charges) {
        self.brokerage += o.brokerage;
        self.stt += o.stt;
        self.exchange += o.exchange;
        self.sebi += o.sebi;
        self.gst += o.gst;
        self.stamp += o.stamp;
        self.dp += o.dp;
    }
}

/// Rates are percents of turnover; JSON spells `_pct` as `_percent`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostModel {
    /// Preset name: none, in-delivery, in-intraday, us
    pub name: String,
    pub brokerage: Brokerage,
    #[serde(rename = "stt_buy_percent")]
    pub stt_buy_pct: f64,
    #[serde(rename = "stt_sell_percent")]
    pub stt_sell_pct: f64,
    #[serde(rename = "exchange_percent")]
    pub exchange_pct: f64,
    #[serde(rename = "sebi_percent")]
    pub sebi_pct: f64,
    #[serde(rename = "stamp_buy_percent")]
    pub stamp_buy_pct: f64,
    /// GST, as a percent of brokerage + exchange + SEBI charges.
    #[serde(rename = "gst_percent")]
    pub gst_pct: f64,
    /// Depository charge per sell order (delivery only).
    pub dp_per_sell: f64,
    /// Adverse price move per fill, in basis points (1 bp = 0.01%).
    pub slippage_bps: f64,
}

/// The `--costs` choice before the stock's currency is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostPreset {
    /// India delivery for INR stocks, the US model for everything else.
    Auto,
    None,
    InDelivery,
    InIntraday,
    Us,
    Crypto,
    Fx,
}

impl CostPreset {
    /// The cost model for `symbol`. `auto` follows the instrument: crypto
    /// and FX get their own models, INR stocks India delivery, the rest US.
    /// NSE equity charges are refused for coins and currency pairs.
    pub fn model_for(self, symbol: &str, currency: Option<&str>) -> Result<CostModel, String> {
        let class = AssetClass::of(symbol);
        let what = match class {
            AssetClass::Crypto => Some(("crypto", "crypto")),
            AssetClass::Fx => Some(("a currency pair", "fx")),
            _ => None,
        };
        Ok(match self {
            CostPreset::Auto => match class {
                AssetClass::Crypto => CostModel::crypto(),
                AssetClass::Fx => CostModel::fx(),
                _ if currency == Some("INR") => CostModel::in_delivery(),
                _ => CostModel::us(),
            },
            CostPreset::InDelivery | CostPreset::InIntraday if what.is_some() => {
                let (kind, preset) = what.expect("checked above");
                return Err(format!(
                    "--costs in-delivery/in-intraday are NSE equity charges (STT, stamp duty, \
                     DP); {symbol} is {kind}. Use --costs {preset}, none or auto"
                ));
            }
            CostPreset::None => CostModel::none(),
            CostPreset::InDelivery => CostModel::in_delivery(),
            CostPreset::InIntraday => CostModel::in_intraday(),
            CostPreset::Us => CostModel::us(),
            CostPreset::Crypto => CostModel::crypto(),
            CostPreset::Fx => CostModel::fx(),
        })
    }
}

/// Default slippage for every preset except `none`.
pub const DEFAULT_SLIPPAGE_BPS: f64 = 5.0;

/// NSE cash-market transaction charge, as a percent of turnover, in force
/// on `date`. NSE raised it from 0.00297% to 0.00307% on 1 March 2026. The
/// presets below keep 0.00297% so existing backtest results do not move;
/// the live order preview uses the rate for the day.
pub fn nse_exchange_pct_on(date: chrono::NaiveDate) -> f64 {
    match chrono::NaiveDate::from_ymd_opt(2026, 3, 1) {
        Some(change) if date >= change => 0.00307,
        _ => 0.00297,
    }
}

impl CostModel {
    pub fn none() -> Self {
        CostModel {
            name: "none".into(),
            brokerage: Brokerage::default(),
            stt_buy_pct: 0.0,
            stt_sell_pct: 0.0,
            exchange_pct: 0.0,
            sebi_pct: 0.0,
            stamp_buy_pct: 0.0,
            gst_pct: 0.0,
            dp_per_sell: 0.0,
            slippage_bps: 0.0,
        }
    }

    /// NSE equity delivery (held overnight or longer).
    pub fn in_delivery() -> Self {
        CostModel {
            name: "in-delivery".into(),
            brokerage: Brokerage {
                flat: Some(20.0),
                pct: Some(0.1),
            },
            stt_buy_pct: 0.1,
            stt_sell_pct: 0.1,
            exchange_pct: 0.00297,
            sebi_pct: 0.0001,
            stamp_buy_pct: 0.015,
            gst_pct: 18.0,
            dp_per_sell: 15.93,
            slippage_bps: DEFAULT_SLIPPAGE_BPS,
        }
    }

    /// NSE equity intraday (bought and sold the same day).
    pub fn in_intraday() -> Self {
        CostModel {
            name: "in-intraday".into(),
            brokerage: Brokerage {
                flat: Some(20.0),
                pct: Some(0.03),
            },
            stt_buy_pct: 0.0,
            stt_sell_pct: 0.025,
            stamp_buy_pct: 0.003,
            dp_per_sell: 0.0,
            ..CostModel::in_delivery()
        }
    }

    /// US equities: commission-free brokerage plus slippage.
    pub fn us() -> Self {
        CostModel {
            name: "us".into(),
            slippage_bps: DEFAULT_SLIPPAGE_BPS,
            ..CostModel::none()
        }
    }

    /// Crypto exchanges: a 0.1% taker fee per order and 10 bps of slippage.
    pub fn crypto() -> Self {
        CostModel {
            name: "crypto".into(),
            brokerage: Brokerage {
                flat: None,
                pct: Some(0.1),
            },
            slippage_bps: 10.0,
            ..CostModel::none()
        }
    }

    /// Spot FX: no commission, 2 bps of slippage for the spread.
    pub fn fx() -> Self {
        CostModel {
            name: "fx".into(),
            slippage_bps: 2.0,
            ..CostModel::none()
        }
    }

    /// Charges for one order of `turnover` (price × quantity).
    pub fn charges(&self, side: Side, turnover: f64) -> Charges {
        let pct = |rate: f64| turnover * rate / 100.0;
        let brokerage = self.brokerage.on(turnover);
        let exchange = pct(self.exchange_pct);
        let sebi = pct(self.sebi_pct);
        let (stt, stamp, dp) = match side {
            Side::Buy => (pct(self.stt_buy_pct), pct(self.stamp_buy_pct), 0.0),
            Side::Sell => (pct(self.stt_sell_pct), 0.0, self.dp_per_sell),
        };
        Charges {
            brokerage,
            stt,
            exchange,
            sebi,
            gst: (brokerage + exchange + sebi) * self.gst_pct / 100.0,
            stamp,
            dp,
        }
    }

    /// The price actually paid or received once slippage moves it against us.
    pub fn fill_price(&self, side: Side, price: f64) -> f64 {
        let slip = self.slippage_bps / 10_000.0;
        match side {
            Side::Buy => price * (1.0 + slip),
            Side::Sell => price * (1.0 - slip),
        }
    }

    /// One-line summary for the report header, listing only the charges
    /// that are actually non-zero (after `--brokerage` / `--slippage-bps`).
    pub fn describe(&self, csym: &str) -> String {
        let kind = match self.name.as_str() {
            "in-delivery" => "India delivery",
            "in-intraday" => "India intraday",
            "us" => "US",
            "crypto" => "Crypto",
            "fx" => "FX",
            other => other,
        };
        let mut parts = Vec::new();
        if self.brokerage != Brokerage::default() {
            let label = if self.name == "crypto" {
                "fee"
            } else {
                "brokerage"
            };
            parts.push(format!("{label} {}", self.brokerage.describe(csym)));
        }
        for (rate, label) in [
            (self.stt_buy_pct + self.stt_sell_pct, "STT"),
            (self.exchange_pct, "exchange"),
            (self.sebi_pct, "SEBI"),
            (self.gst_pct, "GST"),
            (self.stamp_buy_pct, "stamp duty"),
            (self.dp_per_sell, "DP"),
        ] {
            if rate > 0.0 {
                parts.push(label.to_string());
            }
        }
        if self.slippage_bps > 0.0 {
            parts.push(format!("slippage {} bps", self.slippage_bps));
        }
        if parts.is_empty() {
            return format!("{kind} — nothing charged (gross)");
        }
        format!("{kind} — {}", parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// Worked example: buy 100 shares at ₹1,000 and sell them at ₹1,100 on
    /// delivery. Buy turnover ₹1,00,000; sell turnover ₹1,10,000.
    #[test]
    fn india_delivery_worked_example() {
        let m = CostModel::in_delivery();
        let buy = m.charges(Side::Buy, 100_000.0);
        assert!(close(buy.brokerage, 20.0)); // min(₹20, 0.1% = ₹100)
        assert!(close(buy.stt, 100.0)); // 0.1%
        assert!(close(buy.exchange, 2.97)); // 0.00297%
        assert!(close(buy.sebi, 0.10)); // ₹10 per crore
        assert!(close(buy.gst, 0.18 * (20.0 + 2.97 + 0.10))); // ₹4.1526
        assert!(close(buy.stamp, 15.0)); // 0.015%, buy side only
        assert!(close(buy.dp, 0.0));
        assert!(close(buy.total(), 142.2226));

        let sell = m.charges(Side::Sell, 110_000.0);
        assert!(close(sell.brokerage, 20.0));
        assert!(close(sell.stt, 110.0));
        assert!(close(sell.exchange, 3.267));
        assert!(close(sell.sebi, 0.11));
        assert!(close(sell.gst, 0.18 * (20.0 + 3.267 + 0.11))); // ₹4.20786
        assert!(close(sell.stamp, 0.0));
        assert!(close(sell.dp, 15.93));
        assert!(close(sell.total(), 153.51486));

        // ₹10,000 gross profit keeps ₹9,704.26 after charges.
        let net = 10_000.0 - buy.total() - sell.total();
        assert!((net - 9_704.26).abs() < 0.01);
    }

    /// Intraday: buy 100 at ₹1,000, sell at ₹1,010 the same day.
    #[test]
    fn india_intraday_worked_example() {
        let m = CostModel::in_intraday();
        let buy = m.charges(Side::Buy, 100_000.0);
        assert!(close(buy.brokerage, 20.0)); // min(₹20, 0.03% = ₹30)
        assert!(close(buy.stt, 0.0)); // STT on the sell side only
        assert!(close(buy.stamp, 3.0)); // 0.003%
        assert!(close(buy.total(), 30.2226));

        let sell = m.charges(Side::Sell, 101_000.0);
        assert!(close(sell.stt, 25.25)); // 0.025%
        assert!(close(sell.dp, 0.0));
        assert!(close(sell.total(), 52.508826));
    }

    #[test]
    fn nse_exchange_rate_follows_the_date() {
        let d = |y, m, day| chrono::NaiveDate::from_ymd_opt(y, m, day).unwrap();
        assert!(close(nse_exchange_pct_on(d(2026, 2, 28)), 0.00297));
        assert!(close(nse_exchange_pct_on(d(2026, 3, 1)), 0.00307));
        assert!(close(CostModel::in_delivery().exchange_pct, 0.00297));
    }

    #[test]
    fn small_orders_pay_percent_brokerage() {
        // ₹5,000 delivery order: 0.1% = ₹5 is below the ₹20 cap.
        let c = CostModel::in_delivery().charges(Side::Buy, 5_000.0);
        assert!(close(c.brokerage, 5.0));
    }

    #[test]
    fn us_and_none_models() {
        let us = CostModel::us();
        assert!(close(us.charges(Side::Sell, 50_000.0).total(), 0.0));
        assert!(close(us.fill_price(Side::Buy, 100.0), 100.05));
        assert!(close(us.fill_price(Side::Sell, 100.0), 99.95));
        let none = CostModel::none();
        assert!(close(none.fill_price(Side::Buy, 100.0), 100.0));
        assert!(close(none.charges(Side::Buy, 1e6).total(), 0.0));
    }

    #[test]
    fn auto_preset_follows_currency() {
        let auto = |s: &str, c: Option<&str>| CostPreset::Auto.model_for(s, c).unwrap().name;
        assert_eq!(auto("TCS.NS", Some("INR")), "in-delivery");
        assert_eq!(auto("AAPL", Some("USD")), "us");
        assert_eq!(auto("AAPL", None), "us");
        assert_eq!(auto("BTC-INR", Some("INR")), "crypto");
        assert_eq!(auto("USDINR=X", Some("INR")), "fx");
        let e = CostPreset::InDelivery.model_for("BTC-USD", Some("USD"));
        assert!(e.unwrap_err().contains("--costs crypto"));
        assert!(CostPreset::InIntraday.model_for("EURUSD=X", None).is_err());
        assert_eq!(
            CostPreset::Us.model_for("BTC-USD", None).unwrap().name,
            "us"
        );
        let c = CostModel::crypto().charges(Side::Buy, 10_000.0);
        assert!(close(c.total(), 10.0));
        assert_eq!(CostModel::fx().describe("$"), "FX — slippage 2 bps");
    }

    #[test]
    fn descriptions_list_only_what_is_charged() {
        assert_eq!(
            CostModel::none().describe("₹"),
            "none — nothing charged (gross)"
        );
        assert_eq!(CostModel::us().describe("$"), "US — slippage 5 bps");
        let d = CostModel::in_delivery().describe("₹");
        assert!(d.starts_with("India delivery — brokerage ₹20 or 0.1%, lower, STT"));
        assert!(d.ends_with("DP, slippage 5 bps"));
        let mut m = CostModel::none();
        m.brokerage = Brokerage::parse("20").unwrap();
        assert_eq!(m.describe("₹"), "none — brokerage ₹20/order");
    }

    #[test]
    fn brokerage_parsing() {
        assert_eq!(
            Brokerage::parse("20").unwrap(),
            Brokerage {
                flat: Some(20.0),
                pct: None
            }
        );
        assert_eq!(
            Brokerage::parse("0.03%").unwrap(),
            Brokerage {
                flat: None,
                pct: Some(0.03)
            }
        );
        let both = Brokerage::parse("20, 0.03%").unwrap();
        assert!(close(both.on(100_000.0), 20.0));
        assert!(close(both.on(10_000.0), 3.0));
        assert!(Brokerage::parse("abc").is_err());
        assert!(Brokerage::parse("-5").is_err());
        assert!(Brokerage::parse("20,30").is_err());
        assert!(Brokerage::parse("").is_err());
    }
}
