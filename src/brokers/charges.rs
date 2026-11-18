//! Estimated charges and slippage for the live order preview.
//!
//! Statutory rates (STT, exchange transaction charge, SEBI fee, stamp duty,
//! GST) come from the backtest cost model in [`crate::backtest::costs`],
//! with the NSE transaction charge in force on the order's date. This module
//! adds each broker's brokerage and DP charge and a slippage estimate.
//! Everything here is an estimate: the broker's contract note is the final
//! word.

use super::{BrokerKind, OrderKind, Side};
use crate::backtest::costs::{self, Brokerage, Charges, CostModel};
use chrono::NaiveDate;

/// How an Indian cash-equity order is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    /// CNC: held overnight or longer. The product StockWise orders use.
    Delivery,
    /// MIS: bought and sold the same day.
    Intraday,
}

/// One broker's published equity charges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tariff {
    pub delivery: Brokerage,
    pub intraday: Brokerage,
    /// Smallest brokerage on an order that pays any (₹0 for none).
    pub min: f64,
    /// DP charge per delivery sell, GST included.
    pub dp: f64,
}

/// Published equity tariffs (checked November 2026; brokers change them, so
/// the preview labels the result as an estimate). DP charges include 18% GST.
///
/// | Broker    | Delivery            | Intraday            | Minimum | DP per sell     |
/// |-----------|---------------------|---------------------|---------|-----------------|
/// | Angel One | ₹20 or 0.1%, lower  | ₹20 or 0.1%, lower  | ₹5      | ₹20 + GST       |
/// | Zerodha   | ₹0                  | ₹20 or 0.03%, lower | —       | ₹15.34 incl. GST|
/// | Upstox    | ₹20                 | ₹20 or 0.1%, lower  | —       | ₹20 + GST       |
/// | Dhan      | ₹0                  | ₹20 or 0.03%, lower | —       | ₹12.50 + GST    |
/// | Fyers     | ₹20 or 0.3%, lower  | ₹20 or 0.03%, lower | —       | ₹12.50 + GST    |
pub fn tariff(kind: BrokerKind) -> Tariff {
    let capped = |pct: f64| Brokerage {
        flat: Some(20.0),
        pct: Some(pct),
    };
    let flat = |v: f64| Brokerage {
        flat: Some(v),
        pct: None,
    };
    let gst = |v: f64| v * 1.18;
    match kind {
        BrokerKind::Angel => Tariff {
            delivery: capped(0.1),
            intraday: capped(0.1),
            min: 5.0,
            dp: gst(20.0),
        },
        BrokerKind::Zerodha => Tariff {
            delivery: flat(0.0),
            intraday: capped(0.03),
            min: 0.0,
            dp: 15.34,
        },
        BrokerKind::Upstox => Tariff {
            delivery: flat(20.0),
            intraday: capped(0.1),
            min: 0.0,
            dp: gst(20.0),
        },
        BrokerKind::Dhan => Tariff {
            delivery: flat(0.0),
            intraday: capped(0.03),
            min: 0.0,
            dp: gst(12.5),
        },
        BrokerKind::Fyers => Tariff {
            delivery: capped(0.3),
            intraday: capped(0.03),
            min: 0.0,
            dp: gst(12.5),
        },
    }
}

/// `kind`'s brokerage on one `product` order of `turnover`, minimum applied.
pub fn brokerage(kind: BrokerKind, product: Product, turnover: f64) -> f64 {
    let t = tariff(kind);
    let b = match product {
        Product::Delivery => t.delivery,
        Product::Intraday => t.intraday,
    }
    .on(turnover);
    if b > 0.0 { b.max(t.min) } else { b }
}

/// The backtest's NSE cost model for `product` on `date`, with `kind`'s
/// brokerage on `turnover` and its DP charge.
pub fn model(kind: BrokerKind, product: Product, turnover: f64, date: NaiveDate) -> CostModel {
    let mut m = match product {
        Product::Delivery => CostModel::in_delivery(),
        Product::Intraday => CostModel::in_intraday(),
    };
    m.brokerage = Brokerage {
        flat: Some(brokerage(kind, product, turnover)),
        pct: None,
    };
    m.exchange_pct = costs::nse_exchange_pct_on(date);
    if product == Product::Delivery {
        m.dp_per_sell = tariff(kind).dp;
    }
    m
}

fn cost_side(side: Side) -> costs::Side {
    match side {
        Side::Buy => costs::Side::Buy,
        Side::Sell => costs::Side::Sell,
    }
}

/// Charges for one order of `qty` at `price` per share, placed on `date`.
pub fn charges(
    kind: BrokerKind,
    product: Product,
    side: Side,
    qty: u32,
    price: f64,
    date: NaiveDate,
) -> Charges {
    let turnover = price * qty as f64;
    model(kind, product, turnover, date).charges(cost_side(side), turnover)
}

/// Where a slippage estimate comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlippageBasis {
    /// A limit order fills at its limit or better.
    Limit,
    /// Half the quoted bid-ask spread per share.
    Spread { bid: f64, ask: f64 },
    /// No usable bid/ask: the backtest's default slippage, in basis points.
    Default { bps: f64 },
}

/// Estimated cost of the fill moving away from the reference price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slippage {
    pub amount: f64,
    pub basis: SlippageBasis,
}

/// Slippage for `qty` shares of an order of `kind` valued at `price` per
/// share. Market and SL-M orders use half the bid-ask spread when the quote
/// has a sane one, else the backtest default.
pub fn slippage(kind: &OrderKind, qty: u32, price: f64, bid_ask: Option<(f64, f64)>) -> Slippage {
    if matches!(kind, OrderKind::Limit { .. } | OrderKind::StopLimit { .. }) {
        return Slippage {
            amount: 0.0,
            basis: SlippageBasis::Limit,
        };
    }
    let q = qty as f64;
    match bid_ask.filter(|&(b, a)| b.is_finite() && a.is_finite() && b > 0.0 && a >= b) {
        Some((bid, ask)) => Slippage {
            amount: (ask - bid) / 2.0 * q,
            basis: SlippageBasis::Spread { bid, ask },
        },
        None => {
            let m = CostModel::in_delivery();
            Slippage {
                amount: (m.fill_price(costs::Side::Buy, price) - price) * q,
                basis: SlippageBasis::Default {
                    bps: m.slippage_bps,
                },
            }
        }
    }
}

/// The preview's estimate: this order's charges as delivery and, for
/// comparison, as intraday, plus slippage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub delivery: Charges,
    pub intraday: Charges,
    pub slippage: Slippage,
}

/// Estimate charges for an order whose value is `est_value`, placed on `date`.
pub fn estimate(
    broker: BrokerKind,
    side: Side,
    kind: &OrderKind,
    qty: u32,
    est_value: f64,
    bid_ask: Option<(f64, f64)>,
    date: NaiveDate,
) -> Option<Estimate> {
    if qty == 0 || !est_value.is_finite() || est_value <= 0.0 {
        return None;
    }
    let price = est_value / qty as f64;
    Some(Estimate {
        delivery: charges(broker, Product::Delivery, side, qty, price, date),
        intraday: charges(broker, Product::Intraday, side, qty, price, date),
        slippage: slippage(kind, qty, price, bid_ask),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// Before NSE's 1 March 2026 change (exchange charge 0.00297%).
    fn before() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
    }

    /// After it (0.00307%).
    fn after() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 18).unwrap()
    }

    /// Delivery buy on Zerodha: 100 shares at ₹1,000 (turnover ₹1,00,000).
    ///   brokerage  ₹0 (free delivery)
    ///   STT        0.1%      = ₹100
    ///   exchange   0.00297%  = ₹2.97 (0.00307% = ₹3.07 from March 2026)
    ///   SEBI       ₹10/crore = ₹0.10
    ///   stamp      0.015%    = ₹15
    ///   GST        18% × (0 + 2.97 + 0.10) = ₹0.5526 (₹0.5706 after)
    ///   total                = ₹118.6226 (₹118.7406 after)
    #[test]
    fn delivery_buy_worked_example() {
        let z = |d| {
            charges(
                BrokerKind::Zerodha,
                Product::Delivery,
                Side::Buy,
                100,
                1000.0,
                d,
            )
        };
        let c = z(before());
        assert!(close(c.brokerage, 0.0));
        assert!(close(c.stt, 100.0));
        assert!(close(c.exchange, 2.97));
        assert!(close(c.sebi, 0.10));
        assert!(close(c.stamp, 15.0));
        assert!(close(c.gst, 0.5526));
        assert!(close(c.dp, 0.0));
        assert!(close(c.total(), 118.6226));

        let c = z(after());
        assert!(close(c.exchange, 3.07));
        assert!(close(c.gst, 0.5706));
        assert!(close(c.total(), 118.7406));
    }

    /// Delivery sell on Angel One: 100 shares at ₹1,100 (turnover ₹1,10,000).
    ///   brokerage  min(₹20, 0.1% = ₹110) = ₹20
    ///   STT        0.1%      = ₹110
    ///   exchange   0.00307%  = ₹3.377
    ///   SEBI       ₹10/crore = ₹0.11
    ///   stamp      none on a sell
    ///   GST        18% × (20 + 3.377 + 0.11) = ₹4.22766
    ///   DP charge  ₹20 + 18% GST = ₹23.60
    ///   total                = ₹161.31466
    #[test]
    fn delivery_sell_worked_example() {
        let c = charges(
            BrokerKind::Angel,
            Product::Delivery,
            Side::Sell,
            100,
            1100.0,
            after(),
        );
        assert!(close(c.brokerage, 20.0));
        assert!(close(c.stt, 110.0));
        assert!(close(c.exchange, 3.377));
        assert!(close(c.sebi, 0.11));
        assert!(close(c.stamp, 0.0));
        assert!(close(c.gst, 4.22766));
        assert!(close(c.dp, 23.6));
        assert!(close(c.total(), 161.31466));
    }

    /// Intraday round trip on Dhan: buy 100 at ₹1,000, sell 100 at ₹1,010.
    ///   buy:  brokerage min(₹20, ₹30) = ₹20, STT ₹0, exchange 0.00307% =
    ///         ₹3.07, SEBI ₹0.10, stamp 0.003% = ₹3, GST 18% × 23.17 =
    ///         ₹4.1706 → ₹30.3406
    ///   sell: brokerage min(₹20, ₹30.30) = ₹20, STT 0.025% = ₹25.25,
    ///         exchange ₹3.1007, SEBI ₹0.101, GST 18% × 23.2017 = ₹4.176306,
    ///         no DP charge → ₹52.628006
    ///   round trip ₹82.968606
    #[test]
    fn intraday_round_trip_worked_example() {
        let leg = |side, price| {
            charges(
                BrokerKind::Dhan,
                Product::Intraday,
                side,
                100,
                price,
                after(),
            )
        };
        let buy = leg(Side::Buy, 1000.0);
        assert!(close(buy.brokerage, 20.0));
        assert!(close(buy.stt, 0.0));
        assert!(close(buy.exchange, 3.07));
        assert!(close(buy.stamp, 3.0));
        assert!(close(buy.gst, 4.1706));
        assert!(close(buy.total(), 30.3406));

        let sell = leg(Side::Sell, 1010.0);
        assert!(close(sell.brokerage, 20.0));
        assert!(close(sell.stt, 25.25));
        assert!(close(sell.exchange, 3.1007));
        assert!(close(sell.sebi, 0.101));
        assert!(close(sell.stamp, 0.0));
        assert!(close(sell.gst, 4.176306));
        assert!(close(sell.dp, 0.0));
        assert!(close(sell.total(), 52.628006));
        assert!(close(buy.total() + sell.total(), 82.968606));
    }

    #[test]
    fn brokerage_follows_each_broker() {
        let on = |k, p, v| brokerage(k, p, v);
        assert!(close(on(BrokerKind::Zerodha, Product::Delivery, 1e6), 0.0));
        assert!(close(on(BrokerKind::Dhan, Product::Delivery, 1e6), 0.0));
        assert!(close(on(BrokerKind::Angel, Product::Delivery, 1e6), 20.0));
        assert!(close(
            on(BrokerKind::Upstox, Product::Delivery, 500.0),
            20.0
        ));
        // Angel One: 0.1% of ₹1,000 is ₹1, raised to the ₹5 minimum;
        // 0.1% of ₹10,000 is ₹10 for intraday too.
        assert!(close(
            on(BrokerKind::Angel, Product::Delivery, 1_000.0),
            5.0
        ));
        assert!(close(
            on(BrokerKind::Angel, Product::Delivery, 5_000.0),
            5.0
        ));
        assert!(close(
            on(BrokerKind::Angel, Product::Delivery, 8_000.0),
            8.0
        ));
        assert!(close(
            on(BrokerKind::Angel, Product::Intraday, 10_000.0),
            10.0
        ));
        assert!(close(
            on(BrokerKind::Fyers, Product::Delivery, 5_000.0),
            15.0
        ));
        assert!(close(
            on(BrokerKind::Upstox, Product::Intraday, 10_000.0),
            10.0
        ));
        assert!(close(
            on(BrokerKind::Zerodha, Product::Intraday, 10_000.0),
            3.0
        ));
    }

    #[test]
    fn dp_charge_follows_each_broker_on_delivery_sells_only() {
        let dp = |k, p, s| charges(k, p, s, 10, 1000.0, after()).dp;
        assert!(close(
            dp(BrokerKind::Zerodha, Product::Delivery, Side::Sell),
            15.34
        ));
        assert!(close(
            dp(BrokerKind::Fyers, Product::Delivery, Side::Sell),
            14.75
        ));
        assert!(close(
            dp(BrokerKind::Dhan, Product::Delivery, Side::Sell),
            14.75
        ));
        assert!(close(
            dp(BrokerKind::Upstox, Product::Delivery, Side::Sell),
            23.6
        ));
        assert!(close(
            dp(BrokerKind::Fyers, Product::Delivery, Side::Buy),
            0.0
        ));
        assert!(close(
            dp(BrokerKind::Fyers, Product::Intraday, Side::Sell),
            0.0
        ));
    }

    #[test]
    fn slippage_uses_the_spread_then_the_default() {
        // Market buy of 100 with bid ₹999.80 / ask ₹1,000.20: half the
        // ₹0.40 spread per share = ₹20.
        let s = slippage(&OrderKind::Market, 100, 1000.0, Some((999.8, 1000.2)));
        assert!(close(s.amount, 20.0));
        assert!(matches!(s.basis, SlippageBasis::Spread { .. }));
        // No quote (or a crossed one): 5 bps of ₹1,00,000 = ₹50.
        for q in [None, Some((1001.0, 999.0)), Some((0.0, 1000.0))] {
            let s = slippage(&OrderKind::StopMarket { trigger: 990.0 }, 100, 1000.0, q);
            assert!(close(s.amount, 50.0));
            assert_eq!(s.basis, SlippageBasis::Default { bps: 5.0 });
        }
        // Limit and SL orders never fill worse than their limit.
        let s = slippage(&OrderKind::Limit { price: 1000.0 }, 100, 1000.0, None);
        assert_eq!(s.amount, 0.0);
        assert_eq!(s.basis, SlippageBasis::Limit);
    }

    #[test]
    fn estimate_needs_a_value() {
        let k = OrderKind::Market;
        let est = |v| estimate(BrokerKind::Zerodha, Side::Buy, &k, 100, v, None, after());
        assert!(est(0.0).is_none());
        assert!(est(f64::NAN).is_none());
        let e = est(100_000.0).unwrap();
        assert!(close(e.delivery.total(), 118.7406));
        assert!(close(e.intraday.stt, 0.0));
        assert!(close(e.slippage.amount, 50.0));
    }
}
