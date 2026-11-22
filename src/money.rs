//! Money in integer minor units (paise, cents, pence: hundredths of the
//! currency unit). Amounts become `i64` minor units where they enter a sum
//! and turn back into `f64` only for display and JSON, so subtraction and
//! aggregation are exact. Ratios, rates and returns stay `f64`.

/// `amount` in minor units, rounded half away from zero. Non-finite input
/// counts as 0.
pub fn to_minor(amount: f64) -> i64 {
    if amount.is_finite() {
        (amount * 100.0).round() as i64
    } else {
        0
    }
}

/// Minor units back to the currency unit, for display and JSON.
pub fn from_minor(minor: i64) -> f64 {
    minor as f64 / 100.0
}

/// `minor × rate` (an FX rate, a unit scale such as pence to pounds),
/// rounded to the minor unit. The rate stays `f64`.
pub fn times(minor: i64, rate: f64) -> i64 {
    if rate.is_finite() {
        (minor as f64 * rate).round() as i64
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_in_minor_units_are_exact() {
        // 0.1 + 0.2 is not 0.3 in f64; in minor units it is.
        assert_ne!(0.1 + 0.2, 0.3);
        assert_eq!(to_minor(0.1) + to_minor(0.2), to_minor(0.3));
        let total: i64 = std::iter::repeat_n(to_minor(0.01), 1_000).sum();
        assert_eq!(from_minor(total), 10.0);
    }

    #[test]
    fn rounding_and_scaling() {
        assert_eq!(to_minor(1.005_000_1), 101);
        assert_eq!(to_minor(-2.345), -235);
        assert_eq!(to_minor(f64::NAN), 0);
        // 1,234 pence in minor units (123,400) is £12.34 (1,234 minor).
        assert_eq!(times(123_400, 0.01), 1_234);
        assert_eq!(times(100, f64::INFINITY), 0);
    }
}
