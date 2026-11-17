//! Estimated charges and slippage for the live order preview.
//!
//! Statutory rates (STT, exchange transaction charge, SEBI fee, stamp duty,
//! GST, DP charge) come from the backtest cost model in
//! [`crate::backtest::costs`]; this module only adds each broker's
//! brokerage and a slippage estimate. Everything here is an estimate: the
//! broker's contract note is the final word.

use super::{BrokerKind, OrderKind, Side};
use crate::backtest::costs::{self, Brokerage, Charges, CostModel};

/// How an Indian cash-equity order is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    /// CNC: held overnight or longer. The product StockWise orders use.
    Delivery,
    /// MIS: bought and sold the same day.
    Intraday,
}

/// Published equity brokerage per order (rates as of 2025; brokers change
/// them, so the preview labels the result as an estimate).
///
/// | Broker    | Delivery            | Intraday            |
/// |-----------|---------------------|---------------------|
/// | Angel One | ₹20 or 0.1%, lower  | ₹20 or 0.03%, lower |
/// | Zerodha   | ₹0                  | ₹20 or 0.03%, lower |
/// | Upstox    | ₹20 or 2.5%, lower  | ₹20 or 0.1%, lower  |
/// | Dhan      | ₹0                  | ₹20 or 0.03%, lower |
/// | Fyers     | ₹20 or 0.3%, lower  | ₹20 or 0.03%, lower |
pub fn brokerage(kind: BrokerKind, product: Product) -> Brokerage {
    let capped = |pct: f64| Brokerage {
        flat: Some(20.0),
        pct: Some(pct),
    };
    let free = Brokerage {
        flat: Some(0.0),
        pct: None,
    };
    match (kind, product) {
        (BrokerKind::Angel, Product::Delivery) => capped(0.1),
        (BrokerKind::Upstox, Product::Delivery) => capped(2.5),
        (BrokerKind::Fyers, Product::Delivery) => capped(0.3),
        (BrokerKind::Zerodha | BrokerKind::Dhan, Product::Delivery) => free,
        (BrokerKind::Upstox, Product::Intraday) => capped(0.1),
        (_, Product::Intraday) => capped(0.03),
    }
}

/// The backtest's NSE cost model for `product`, with `kind`'s brokerage.
pub fn model(kind: BrokerKind, product: Product) -> CostModel {
    let mut m = match product {
        Product::Delivery => CostModel::in_delivery(),
        Product::Intraday => CostModel::in_intraday(),
    };
    m.brokerage = brokerage(kind, product);
    m
}

fn cost_side(side: Side) -> costs::Side {
    match side {
        Side::Buy => costs::Side::Buy,
        Side::Sell => costs::Side::Sell,
    }
}

/// Charges for one order of `qty` at `price` per share.
pub fn charges(kind: BrokerKind, product: Product, side: Side, qty: u32, price: f64) -> Charges {
    model(kind, product).charges(cost_side(side), price * qty as f64)
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

/// Estimate charges for an order whose value is `est_value`.
pub fn estimate(
    broker: BrokerKind,
    side: Side,
    kind: &OrderKind,
    qty: u32,
    est_value: f64,
    bid_ask: Option<(f64, f64)>,
) -> Option<Estimate> {
    if qty == 0 || !est_value.is_finite() || est_value <= 0.0 {
        return None;
    }
    let price = est_value / qty as f64;
    Some(Estimate {
        delivery: charges(broker, Product::Delivery, side, qty, price),
        intraday: charges(broker, Product::Intraday, side, qty, price),
        slippage: slippage(kind, qty, price, bid_ask),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// Delivery buy on Zerodha: 100 shares at ₹1,000 (turnover ₹1,00,000).
    ///   brokerage  ₹0 (free delivery)
    ///   STT        0.1%      = ₹100
    ///   exchange   0.00297%  = ₹2.97
    ///   SEBI       ₹10/crore = ₹0.10
    ///   stamp      0.015%    = ₹15
    ///   GST        18% × (0 + 2.97 + 0.10) = ₹0.5526
    ///   total                = ₹118.6226
    #[test]
    fn delivery_buy_worked_example() {
        let c = charges(
            BrokerKind::Zerodha,
            Product::Delivery,
            Side::Buy,
            100,
            1000.0,
        );
        assert!(close(c.brokerage, 0.0));
        assert!(close(c.stt, 100.0));
        assert!(close(c.exchange, 2.97));
        assert!(close(c.sebi, 0.10));
        assert!(close(c.stamp, 15.0));
        assert!(close(c.gst, 0.5526));
        assert!(close(c.dp, 0.0));
        assert!(close(c.total(), 118.6226));
    }

    /// Delivery sell on Angel One: 100 shares at ₹1,100 (turnover ₹1,10,000).
    ///   brokerage  min(₹20, 0.1% = ₹110) = ₹20
    ///   STT        0.1%      = ₹110
    ///   exchange   0.00297%  = ₹3.267
    ///   SEBI       ₹10/crore = ₹0.11
    ///   stamp      none on a sell
    ///   GST        18% × (20 + 3.267 + 0.11) = ₹4.20786
    ///   DP charge  ₹15.93
    ///   total                = ₹153.51486
    #[test]
    fn delivery_sell_worked_example() {
        let c = charges(
            BrokerKind::Angel,
            Product::Delivery,
            Side::Sell,
            100,
            1100.0,
        );
        assert!(close(c.brokerage, 20.0));
        assert!(close(c.stt, 110.0));
        assert!(close(c.exchange, 3.267));
        assert!(close(c.sebi, 0.11));
        assert!(close(c.stamp, 0.0));
        assert!(close(c.gst, 4.20786));
        assert!(close(c.dp, 15.93));
        assert!(close(c.total(), 153.51486));
    }

    /// Intraday round trip on Dhan: buy 100 at ₹1,000, sell 100 at ₹1,010.
    ///   buy:  brokerage min(₹20, ₹30) = ₹20, STT ₹0, exchange ₹2.97,
    ///         SEBI ₹0.10, stamp 0.003% = ₹3, GST 18% × 23.07 = ₹4.1526
    ///         → ₹30.2226
    ///   sell: brokerage min(₹20, ₹30.30) = ₹20, STT 0.025% = ₹25.25,
    ///         exchange ₹2.9997, SEBI ₹0.101, GST 18% × 23.1007 = ₹4.158126,
    ///         no DP charge → ₹52.508826
    ///   round trip ₹82.731426
    #[test]
    fn intraday_round_trip_worked_example() {
        let buy = charges(BrokerKind::Dhan, Product::Intraday, Side::Buy, 100, 1000.0);
        assert!(close(buy.brokerage, 20.0));
        assert!(close(buy.stt, 0.0));
        assert!(close(buy.stamp, 3.0));
        assert!(close(buy.gst, 4.1526));
        assert!(close(buy.total(), 30.2226));

        let sell = charges(BrokerKind::Dhan, Product::Intraday, Side::Sell, 100, 1010.0);
        assert!(close(sell.brokerage, 20.0));
        assert!(close(sell.stt, 25.25));
        assert!(close(sell.exchange, 2.9997));
        assert!(close(sell.sebi, 0.101));
        assert!(close(sell.stamp, 0.0));
        assert!(close(sell.gst, 4.158126));
        assert!(close(sell.dp, 0.0));
        assert!(close(sell.total(), 52.508826));
        assert!(close(buy.total() + sell.total(), 82.731426));
    }

    #[test]
    fn brokerage_follows_each_broker() {
        let on = |k, p, v| brokerage(k, p).on(v);
        assert!(close(on(BrokerKind::Zerodha, Product::Delivery, 1e6), 0.0));
        assert!(close(on(BrokerKind::Dhan, Product::Delivery, 1e6), 0.0));
        assert!(close(on(BrokerKind::Angel, Product::Delivery, 1e6), 20.0));
        // Small orders pay the percent: 0.1% of ₹5,000, 2.5% of ₹500.
        assert!(close(
            on(BrokerKind::Angel, Product::Delivery, 5_000.0),
            5.0
        ));
        assert!(close(
            on(BrokerKind::Upstox, Product::Delivery, 500.0),
            12.5
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
        assert!(estimate(BrokerKind::Zerodha, Side::Buy, &k, 10, 0.0, None).is_none());
        assert!(estimate(BrokerKind::Zerodha, Side::Buy, &k, 10, f64::NAN, None).is_none());
        let e = estimate(BrokerKind::Zerodha, Side::Buy, &k, 100, 100_000.0, None).unwrap();
        assert!(close(e.delivery.total(), 118.6226));
        assert!(close(e.intraday.stt, 0.0));
        assert!(close(e.slippage.amount, 50.0));
    }
}
