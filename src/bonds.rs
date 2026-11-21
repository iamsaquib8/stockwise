//! Bond math for Indian government securities: price ↔ yield-to-maturity
//! for coupon G-secs and the discount yield of Treasury bills.
//!
//! Conventions (as used for G-secs in India):
//!
//! * Coupons are paid half-yearly, on the maturity date's day of the month.
//! * Day count is 30/360 (European: the 31st counts as the 30th).
//! * Yields compound at the coupon frequency; in the final coupon period the
//!   price discounts with simple interest, like spreadsheet `PRICE`/`YIELD`.
//! * Prices are per 100 of face value. The clean price excludes accrued
//!   interest; the dirty price is what a buyer pays.
//! * T-bills are zero-coupon discount instruments:
//!   `yield = (100 − price) / price × 365 / days`.
//!
//! Everything here is a pure function of its inputs, so it is unit-tested
//! against hand-checked values and needs no network.

use anyhow::{Result, bail};
use chrono::{Datelike, Months, NaiveDate};

/// Face value prices are quoted per.
pub const FACE: f64 = 100.0;

/// A fixed-coupon bond.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bond {
    /// Annual coupon rate in percent (7.18 for a 7.18% G-sec).
    pub coupon_pct: f64,
    pub maturity: NaiveDate,
    /// Coupons a year: 2 for G-secs.
    pub frequency: u32,
}

/// Everything `bonds calc` prints for a bond at one settlement date.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Analytics {
    pub clean_price: f64,
    pub dirty_price: f64,
    pub accrued: f64,
    /// Yield to maturity in percent, at the coupon frequency.
    pub ytm_pct: f64,
    /// Annual coupon over clean price, in percent.
    pub current_yield_pct: f64,
    /// Macaulay duration in years.
    pub macaulay_years: f64,
    /// Modified duration: % price change for a 1-point (100 bp) yield move.
    pub modified_duration: f64,
    pub next_coupon: NaiveDate,
    pub coupons_left: u32,
}

/// Days between two dates under 30E/360.
pub fn days_30e_360(from: NaiveDate, to: NaiveDate) -> i64 {
    let d1 = from.day().min(30) as i64;
    let d2 = to.day().min(30) as i64;
    360 * (to.year() - from.year()) as i64
        + 30 * (to.month() as i64 - from.month() as i64)
        + (d2 - d1)
}

impl Bond {
    pub fn new(coupon_pct: f64, maturity: NaiveDate) -> Result<Self> {
        if !coupon_pct.is_finite() || !(0.0..=50.0).contains(&coupon_pct) {
            bail!("coupon must be between 0 and 50 percent, got {coupon_pct}");
        }
        Ok(Bond {
            coupon_pct,
            maturity,
            frequency: 2,
        })
    }

    fn months_per_period(&self) -> u32 {
        12 / self.frequency
    }

    /// The coupon date `n` periods before maturity (0 = maturity).
    fn coupon_date(&self, n: u32) -> NaiveDate {
        self.maturity
            .checked_sub_months(Months::new(n * self.months_per_period()))
            .expect("coupon dates stay in range")
    }

    /// (previous coupon date, next coupon date, coupons left) at `settle`.
    fn schedule(&self, settle: NaiveDate) -> Result<(NaiveDate, NaiveDate, u32)> {
        if settle >= self.maturity {
            bail!(
                "settlement {} is on or after maturity {}",
                settle,
                self.maturity
            );
        }
        // Walk back from maturity until the coupon date is on or before settlement.
        let mut n = 0;
        while self.coupon_date(n + 1) > settle {
            n += 1;
        }
        Ok((self.coupon_date(n + 1), self.coupon_date(n), n + 1))
    }

    fn coupon_per_period(&self) -> f64 {
        FACE * self.coupon_pct / 100.0 / self.frequency as f64
    }

    /// Accrued interest per 100 face at `settle`.
    pub fn accrued(&self, settle: NaiveDate) -> Result<f64> {
        let (prev, _, _) = self.schedule(settle)?;
        let e = 360.0 / self.frequency as f64;
        let a = days_30e_360(prev, settle) as f64;
        Ok(self.coupon_per_period() * a / e)
    }

    /// Dirty price per 100 face for a yield (percent) at `settle`.
    pub fn dirty_price(&self, settle: NaiveDate, ytm_pct: f64) -> Result<f64> {
        let (_, next, n) = self.schedule(settle)?;
        let f = self.frequency as f64;
        let y = ytm_pct / 100.0;
        let e = 360.0 / f;
        let dsc = days_30e_360(settle, next) as f64;
        let c = self.coupon_per_period();
        if n == 1 {
            // Final period: simple interest to maturity.
            let discount = 1.0 + dsc / e * y / f;
            if discount <= 0.0 {
                bail!("yield {ytm_pct}% is too negative to price");
            }
            return Ok((FACE + c) / discount);
        }
        let base = 1.0 + y / f;
        if base <= 0.0 {
            bail!("yield {ytm_pct}% is below −{}%", 100.0 * f);
        }
        let w = dsc / e;
        let mut pv = 0.0;
        for k in 0..n {
            pv += c / base.powf(k as f64 + w);
        }
        pv += FACE / base.powf((n - 1) as f64 + w);
        Ok(pv)
    }

    /// Clean price per 100 face for a yield (percent) at `settle`.
    pub fn clean_price(&self, settle: NaiveDate, ytm_pct: f64) -> Result<f64> {
        Ok(self.dirty_price(settle, ytm_pct)? - self.accrued(settle)?)
    }

    /// Yield to maturity (percent) for a clean price at `settle`, by
    /// bisection: the price falls as the yield rises, so the root is unique.
    pub fn ytm(&self, settle: NaiveDate, clean_price: f64) -> Result<f64> {
        if !clean_price.is_finite() || clean_price <= 0.0 {
            bail!("price must be positive, got {clean_price}");
        }
        let price_at = |y: f64| self.clean_price(settle, y);
        // −20% … 200%: far wider than any traded G-sec.
        let (mut lo, mut hi) = (-20.0_f64, 200.0_f64);
        let (p_lo, p_hi) = (price_at(lo)?, price_at(hi)?);
        if clean_price > p_lo || clean_price < p_hi {
            bail!("price {clean_price:.4} is outside what any yield between {lo}% and {hi}% gives");
        }
        for _ in 0..200 {
            let mid = (lo + hi) / 2.0;
            if price_at(mid)? > clean_price {
                lo = mid;
            } else {
                hi = mid;
            }
            if hi - lo < 1e-10 {
                break;
            }
        }
        Ok((lo + hi) / 2.0)
    }

    /// Price, yield, accrued interest and duration at `settle` for a yield.
    pub fn analytics_at_yield(&self, settle: NaiveDate, ytm_pct: f64) -> Result<Analytics> {
        let dirty = self.dirty_price(settle, ytm_pct)?;
        let accrued = self.accrued(settle)?;
        let (_, next, n) = self.schedule(settle)?;
        let clean = dirty - accrued;
        let (macaulay, modified) = self.durations(settle, ytm_pct, dirty)?;
        Ok(Analytics {
            clean_price: clean,
            dirty_price: dirty,
            accrued,
            ytm_pct,
            current_yield_pct: self.coupon_pct / clean * FACE,
            macaulay_years: macaulay,
            modified_duration: modified,
            next_coupon: next,
            coupons_left: n,
        })
    }

    /// The same for a clean price.
    pub fn analytics_at_price(&self, settle: NaiveDate, clean_price: f64) -> Result<Analytics> {
        let y = self.ytm(settle, clean_price)?;
        let mut a = self.analytics_at_yield(settle, y)?;
        // Report the price the user gave, not the bisection's round trip.
        a.clean_price = clean_price;
        a.dirty_price = clean_price + a.accrued;
        a.current_yield_pct = self.coupon_pct / clean_price * FACE;
        Ok(a)
    }

    /// (Macaulay years, modified duration) from the cash flows.
    fn durations(&self, settle: NaiveDate, ytm_pct: f64, dirty: f64) -> Result<(f64, f64)> {
        let (_, next, n) = self.schedule(settle)?;
        let f = self.frequency as f64;
        let y = ytm_pct / 100.0;
        let base = 1.0 + y / f;
        let w = days_30e_360(settle, next) as f64 / (360.0 / f);
        let c = self.coupon_per_period();
        let mut weighted = 0.0;
        for k in 0..n {
            let t = k as f64 + w; // in periods
            let cash = if k + 1 == n { c + FACE } else { c };
            weighted += t * cash / base.powf(t);
        }
        let macaulay = weighted / dirty / f;
        Ok((macaulay, macaulay / base))
    }
}

/// Treasury-bill yield (percent) from a price per 100 and days to maturity.
pub fn tbill_yield(price: f64, days: i64) -> Result<f64> {
    if days <= 0 {
        bail!("the bill has matured ({days} days left)");
    }
    if !price.is_finite() || price <= 0.0 || price > FACE {
        bail!("a T-bill price is between 0 and 100, got {price}");
    }
    Ok((FACE - price) / price * 365.0 / days as f64 * 100.0)
}

/// Treasury-bill price per 100 from a yield (percent) and days to maturity.
pub fn tbill_price(yield_pct: f64, days: i64) -> Result<f64> {
    if days <= 0 {
        bail!("the bill has matured ({days} days left)");
    }
    if !yield_pct.is_finite() || yield_pct <= -100.0 {
        bail!("yield must be above −100%, got {yield_pct}");
    }
    // A long bill at a deeply negative yield would divide by zero or less.
    let discount = 1.0 + yield_pct / 100.0 * days as f64 / 365.0;
    if discount <= 0.0 {
        bail!("a {yield_pct}% yield over {days} days gives no valid T-bill price");
    }
    Ok(FACE / discount)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn tbill_price_refuses_a_non_positive_discount_factor() {
        assert!(tbill_price(-60.0, 730).is_err());
        assert!(tbill_price(-99.0, 365).is_ok_and(|p| p.is_finite() && p > 0.0));
        assert!(tbill_price(-100.0 * 365.0 / 400.0, 400).is_err());
    }

    #[test]
    fn day_count_is_30e_360() {
        assert_eq!(days_30e_360(date(2026, 1, 15), date(2026, 7, 15)), 180);
        assert_eq!(days_30e_360(date(2026, 1, 31), date(2026, 3, 31)), 60);
        assert_eq!(days_30e_360(date(2026, 2, 28), date(2026, 3, 31)), 32);
        assert_eq!(days_30e_360(date(2025, 12, 15), date(2026, 1, 15)), 30);
    }

    #[test]
    fn at_par_the_yield_equals_the_coupon_on_a_coupon_date() {
        let bond = Bond::new(7.18, date(2033, 8, 14)).unwrap();
        let settle = date(2026, 8, 14);
        let price = bond.clean_price(settle, 7.18).unwrap();
        assert!(close(price, 100.0, 1e-9), "{price}");
        assert_eq!(bond.accrued(settle).unwrap(), 0.0);
        let y = bond.ytm(settle, 100.0).unwrap();
        assert!(close(y, 7.18, 1e-7), "{y}");
    }

    #[test]
    fn price_and_yield_round_trip_between_coupon_dates() {
        let bond = Bond::new(6.79, date(2034, 10, 7)).unwrap();
        let settle = date(2026, 10, 26);
        for y in [5.0, 6.5, 6.79, 7.2, 9.0] {
            let p = bond.clean_price(settle, y).unwrap();
            let back = bond.ytm(settle, p).unwrap();
            assert!(close(back, y, 1e-7), "{y} → {p} → {back}");
        }
        // Higher yield, lower price.
        assert!(bond.clean_price(settle, 7.0).unwrap() < bond.clean_price(settle, 6.0).unwrap());
    }

    #[test]
    fn hand_checked_price() {
        // 10% semi-annual bond, exactly 2 years (4 coupons) to maturity at
        // 8%: Σ 5/1.04^k (k=1..4) + 100/1.04^4 = 103.6299.
        let bond = Bond::new(10.0, date(2028, 6, 15)).unwrap();
        let settle = date(2026, 6, 15);
        let p = bond.clean_price(settle, 8.0).unwrap();
        assert!(close(p, 103.629_895, 1e-5), "{p}");
        // Accrued interest after 90 of 180 days: half a coupon.
        let mid = date(2026, 9, 15);
        assert!(close(bond.accrued(mid).unwrap(), 2.5, 1e-12));
        let a = bond.analytics_at_yield(mid, 8.0).unwrap();
        assert!(close(a.dirty_price - a.clean_price, 2.5, 1e-12));
        assert_eq!(a.next_coupon, date(2026, 12, 15));
        assert_eq!(a.coupons_left, 4);
    }

    #[test]
    fn final_period_uses_simple_interest() {
        let bond = Bond::new(8.0, date(2026, 12, 1)).unwrap();
        let settle = date(2026, 9, 1); // 90 days of 180 left
        let dirty = bond.dirty_price(settle, 6.0).unwrap();
        // (100 + 4) / (1 + 0.5 × 0.03)
        assert!(close(dirty, 104.0 / 1.015, 1e-9), "{dirty}");
        let y = bond
            .ytm(settle, bond.clean_price(settle, 6.0).unwrap())
            .unwrap();
        assert!(close(y, 6.0, 1e-7));
    }

    #[test]
    fn duration_of_a_zero_coupon_bond_is_its_life() {
        let bond = Bond::new(0.0, date(2031, 4, 1)).unwrap();
        let a = bond.analytics_at_yield(date(2026, 4, 1), 7.0).unwrap();
        assert!(close(a.macaulay_years, 5.0, 1e-9), "{}", a.macaulay_years);
        assert!(close(a.modified_duration, 5.0 / 1.035, 1e-9));
        // Coupons shorten it.
        let coupon = Bond::new(7.0, date(2031, 4, 1)).unwrap();
        let c = coupon.analytics_at_yield(date(2026, 4, 1), 7.0).unwrap();
        assert!(c.macaulay_years < 5.0 && c.macaulay_years > 4.0);
        assert!(close(c.clean_price, 100.0, 1e-9));
        assert!(close(c.current_yield_pct, 7.0, 1e-9));
    }

    #[test]
    fn analytics_at_price_keeps_the_given_price() {
        let bond = Bond::new(7.1, date(2034, 4, 8)).unwrap();
        let a = bond.analytics_at_price(date(2026, 10, 26), 101.25).unwrap();
        assert_eq!(a.clean_price, 101.25);
        assert!(a.ytm_pct < 7.1, "premium bond yields less than its coupon");
        assert!(close(
            bond.clean_price(date(2026, 10, 26), a.ytm_pct).unwrap(),
            101.25,
            1e-6
        ));
    }

    #[test]
    fn bad_inputs_are_errors() {
        assert!(Bond::new(-1.0, date(2030, 1, 1)).is_err());
        let bond = Bond::new(7.0, date(2026, 1, 1)).unwrap();
        assert!(bond.ytm(date(2026, 1, 1), 100.0).is_err());
        assert!(bond.ytm(date(2025, 1, 1), 0.0).is_err());
        assert!(tbill_yield(101.0, 91).is_err());
        assert!(tbill_yield(98.0, 0).is_err());
    }

    #[test]
    fn tbill_yields() {
        // 91-day bill at 98.50: (1.5 / 98.5) × 365 / 91 = 6.1081%.
        let y = tbill_yield(98.5, 91).unwrap();
        assert!(close(y, 6.108_105, 1e-5), "{y}");
        let p = tbill_price(y, 91).unwrap();
        assert!(close(p, 98.5, 1e-9));
        // 364-day bill at 6.5%.
        let p = tbill_price(6.5, 364).unwrap();
        assert!(close(p, 100.0 / (1.0 + 0.065 * 364.0 / 365.0), 1e-12));
    }
}
